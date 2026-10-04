# rverdict — plan

A native Rust "System One" decision engine: give it some text (the state)
and typed questions, and it returns typed answers with calibrated
probabilities from one forward pass, without generating any text. It ships as
an embeddable library, a CLI and a server, and its training pipeline is
built in from the start. Post Office is the first consumer and uses it as its
main classification engine.

Status: baseline agreed 2026-10-03 (see section 9). Phases 0–2 complete; see
`research/phase-0-spike.md`, `phase-1-engine.md` and `phase-2-calibration.md`.

---

## 1. Goals and non-goals

### Goals
- **Rust end to end**: inference, training, calibration, evaluation and data
  preparation. Python is not needed at any stage.
- **Embeddable**: a library crate that Post Office links in-process, with no
  sidecar, no separate server and no Python.
- **Jev-compatible request format** (`POST /v1/systemone`, primitives
  `noul` / `choice` / `score`). That keeps it interchangeable with Jev,
  laya-serve, strands-decider and von.
- **Training from day one**: fine-tune on public decision data, distil from
  an LLM teacher, and specialise on personal Post Office data. It should
  eventually be able to train its own base checkpoint.
- **Measured quality**: every model change is scored by the same evaluation
  harness, using paired significance tests, before it is promoted.
- **Portable, open source, for everyone**: one codebase that runs on any
  reasonable machine: a GPU through wgpu (Vulkan, Metal, DX12), CUDA or ROCm
  where present, and any CPU otherwise. The backend is chosen at runtime,
  falls back safely, and nothing is tuned to one machine. Performance is
  published per hardware profile, never as a single number.
- **Reference machine** (where Phase 0 was measured, not a target): AMD
  Ryzen AI 9 HX 370 with a Radeon 890M iGPU and 64 GB of memory.

### Non-goals (for now)
- Text generation, chat or summarisation. Those stay with LLMs.
- Languages other than English.
- Decoder-based deciders built on Qwen3.5's hybrid attention, such as
  Strands Decider. That would be a later backend if it ever earns its place.
- Training ever blocking Post Office. Post Office ships on open weights (Von)
  while rverdict's own models are trained alongside.

---

## 2. Repository

Separate repo: `rverdict` (crates.io names `rverdict` and `rverdict-*` are
free, checked 2026-10-03). Licence: Apache-2.0 OR MIT, compatible with the
Apache-2.0 upstream weights (Von, Laya, ModernBERT).

```
rverdict/
  crates/
    rverdict-core      wire types, Question/Answer, DecisionModel trait,
                       prompt rendering contract, confidence math,
                       calibration maps, error types. No ML deps.
    rverdict-model     the network: ModernBERT encoder + option-marker head,
                       generic over the framework backend. Weight loading
                       (safetensors, PyTorch .pt import).
    rverdict-engine    inference engine: tokenisation, batching all questions
                       for one state, independent-options masking, truncation
                       policy, thread pool, warm model cache.
    rverdict-train     training loop, losses, optimiser/schedules, data
                       pipeline (JSONL + parquet), option shuffling, ordinal
                       smoothing, checkpointing, LoRA (optional), distillation.
    rverdict-eval      metrics (accuracy, NLL, Brier, ECE, reliability curve),
                       paired McNemar, benchmark runners, temperature fitting.
    rverdict-data      dataset builders: HF Hub download (hf-hub crate),
                       parquet → decision rows, synthetic generation via any
                       OpenAI-compatible LLM, teacher labelling.
    rverdict-remote    client for any /v1/systemone-compatible API: TypeSafe
                       Jev cloud, Cloudflare Workers AI (Clef, Clef-flash,
                       Jev), laya-serve, strands-decider, rverdict-server.
    rverdict-server    axum server exposing /v1/systemone.
    rverdict-cli       `rverdict ask | serve | eval | calibrate | train |
                       data | export`.
  models/              (gitignored) downloaded checkpoints
  data/                recipes + small committed fixtures; built corpora gitignored
  research/            pre-registered experiments and results log
  docs/                architecture, training, wire format, writing questions
```

`rverdict-core` stays dependency-light so Post Office can depend on it
without pulling in an ML framework unless the `embedded` feature is on.

---

## 3. Key decisions

### 3.1 ML framework: Burn (confirmed in Phase 0)

| | Burn | candle |
|---|---|---|
| Training | First-class: autodiff, optimisers, learner, checkpointing | Possible (autograd, AdamW, candle-lora) but low-level |
| AMD GPU | **Yes**: wgpu/Vulkan and ROCm/HIP via CubeCL | **No** (CPU, CUDA, Metal only) |
| CPU | ndarray / candle backend | Strong |
| ModernBERT | Not available; port it (~600–900 LOC) | In candle-transformers; laya-rust has a working fork |
| PyTorch `.pt` import | burn-import `PyTorchFileRecorder` | `candle_core::pickle` |

Training is in scope from the start and the project must run on every
vendor's hardware, so **Burn is the framework**: one model definition for
inference and training, over CUDA, ROCm, wgpu (Vulkan/Metal/DX12) and CPU.
Phase 0 confirmed it on the reference machine (`research/phase-0-spike.md`):

- The Burn ModernBERT port matches candle-transformers to ~3e-6 mean absolute
  error on CPU, and Von's `option_marker.pt` loads directly.
- On the reference iGPU (Vulkan): 91 ms for a 125-token question, 339 ms at
  512 tokens, ≈ 570 tokens/s for full AdamW fine-tuning.
- On the reference CPU (Flex): about 7× slower than its iGPU at inference,
  10× at training. burn-ndarray and burn-cpu are slower still; burn-candle is
  deprecated upstream.
- candle-transformers stays only as the parity reference in tests.

Model code is written against Burn's `Backend` trait. Burn must be built with
its backend default features (rayon/SIMD, autotune, fusion); without them
Phase 0 measured Flex 2× and Vulkan 5× slower.

### 3.1a Backends, runtime selection and distribution

| Backend | Covers | Role |
|---|---|---|
| **wgpu** | Vulkan (Linux, Windows, Android), Metal (macOS) | Default GPU path, one build feature per OS. Vulkan measured in Phase 0; Metal shares the code |
| **CUDA** (`burn-cuda`) | NVIDIA | Optional feature, usually fastest on NVIDIA |
| **ROCm** (`burn-rocm`) | AMD discrete GPUs | Optional feature |
| **Flex** (`burn-flex`) | Any CPU: x86-64, ARM64, wasm | Always compiled; the fallback |

- **Runtime selection** (decided in Phase 1): the engine uses Burn's
  `Dispatch` backend, one concrete backend type whose device is picked at
  runtime, with no measurable overhead. Order: CUDA if compiled in and a
  driver reports a device, ROCm, then each hardware wgpu adapter (discrete
  before integrated, found by enumerating adapters first, because asking
  cubecl for an absent adapter type crashes inside the driver), then Flex.
  Software adapters such as lavapipe are only used when asked for
  explicitly. Override with `RVERDICT_BACKEND=auto|cpu|wgpu|cuda|rocm`.
- **Startup self-test.** A tiny model runs on the candidate with the CPU's
  weights and must match the CPU's logits; a failure or mismatch (driver bugs
  exist) moves on to the next candidate and records why.
- **CUDA without a driver:** CubeCL loads CUDA dynamically
  (`cudarc` with `fallback-dynamic-loading`), so a CUDA build compiles
  without the toolkit and starts on machines without NVIDIA drivers; checked
  in CI by compilation and by hand per `docs/backend-checklist.md`.
- **Precision.** f32 by default; f16 opt-in (`--f16`), with layer norms kept
  in f32 because f16 norms broke parity on Vulkan. f16 halves weight memory
  (≈ 1.6 GB → 0.8 GB) and, with autotune off, runs about 3× faster than f32 on
  the reference iGPU with identical predictions. It becomes the GPU default
  once the self-test covers f16 per device.
- **No autotune, no fusion** (Phase 2): Burn's autotune replayed a kernel
  choice onto shapes it computed wrongly on Vulkan; fusion is correct without
  autotune but no faster. Attention is explicit matmul → softmax → matmul in
  blocks of 512 queries, which keeps every GPU launch under driver timeouts.
  GPU correctness is checked with `rverdict eval compare` on long inputs, not
  only JevBench.
- **Caches.** Converted weights (content-addressed by the source blob's hash)
  and cubecl's compiled kernels (keyed by the running binary) live under the rverdict
  cache (`RVERDICT_CACHE`), so GPU warm-up is paid once per machine.
- **Distribution.** Release binaries for Linux, macOS and Windows built with
  wgpu and Flex, plus a CUDA build if it cannot safely live in the main one.
  Cargo features for building from source. Post Office follows the same
  matrix.
- **Testing without every GPU.** A tiny ModernBERT (2 layers, hidden 64,
  random weights) gives cross-backend parity tests that run in seconds. CI:
  Linux with Flex and Vulkan through Mesa's lavapipe software driver, macOS
  building Metal (GitHub's macOS runners have no usable GPU), Windows on
  CPU; CUDA compile-checked in CI; real GPUs, CUDA and ROCm run by hand
  against `docs/backend-checklist.md`. The full 395M candle parity check stays an on-demand
  test.
- **Benchmarks.** `rverdict bench` measures any machine (inference latency
  by length, training throughput, memory) and prints a hardware profile;
  community results go into a hardware table in `docs/`.

### 3.2 Model family: encoder with option markers

The approach Von and Laya use, chosen because it is:
- **Bidirectional**: every token sees the whole input, which suits
  classification better than causal decoders.
- **Small**: ModernBERT-large has 395M parameters (base: 149M). It is
  practical on CPU and can be trained locally.
- **Long-context**: 8192 tokens, enough for nearly all emails.
- **Supported by existing tooling**: Von's weights (Apache-2.0) give a
  working baseline on day one.

Prompt rendering matches the Von weights (confirmed in Phase 0):
```
[CLS] <instructions> <state> [SEP] [MASK] <option 1> [MASK] <option 2> … [SEP]
```
`[MASK]` and `[SEP]` literals inside user text are broken with a zero-width
joiner so an email cannot forge structure.
Each option's `[MASK]` hidden state goes to a small scorer that produces one
logit; a softmax over a question's options gives the answer.

**Independent-options attention** (Von 1.2's lesson): option tokens attend
only to the shared premise and to their own tokens, and position ids restart
per option. The result is exactly order-invariant: permuting the options
permutes the logits and nothing else.

### 3.3 Confidence and calibration (shared by every backend)
- `choice` confidence = `(N·p_max − 1)/(N − 1)`, which does not depend on N.
- `score` = expected level; confidence = `1 − σ/σ_max`, with a floor
  correction for ordinal smoothing.
- `noul` = P(true).
- Calibration map: one temperature per (type, option-count bucket), plus
  optionally Von's input-conditioned form
  (`T = b + w_H·H_norm + w_len·log10(tokens)/4 + w_K·K/8`). Temperature is
  monotonic, so it never changes the answer, only the confidence.
- `rverdict calibrate labels.jsonl` refits the map with the model weights
  frozen, in minutes on CPU.

### 3.4 Wire format
Jev-compatible request and response (`state`, `questions{id: {type,
instructions, criteria}}`, `answers{id: {choice|noul|score, probabilities,
confidence}}`, `usage`). Unknown fields are ignored and malformed questions
return 422 with the reason.

---

## 4. Training (built in from the start)

### 4.1 Training ladder (each rung is useful on its own)
1. **Calibration only**: refit temperatures on labelled data. Cheap and an
   immediate improvement.
2. **Head + LoRA fine-tune** starting from Von (or Laya-typed) weights on
   domain data. Hours on CPU, faster on the GPU.
3. **Full fine-tune** of the encoder and head from Von weights.
4. **Own base checkpoint**: start from `answerdotai/ModernBERT-large` and
   train the option-marker head and encoder on the rverdict corpus. This is
   "our model", and later releases improve on it.
5. Optional **fast tier**: ModernBERT-base (149M) distilled from the large
   model, for very low latency.

### 4.2 Data sources (all built in Rust by `rverdict-data`)
- **Public decision tasks** downloaded from the HF Hub as parquet: intent
  (Banking77, CLINC, MASSIVE-en), topic (AG News), sentiment and emotion
  (SST, dair-ai/emotion), entailment (MNLI, ANLI, WANLI), paraphrase (PAWS,
  QQP), multi-step (ContractNLI, MuSiQue). Each is converted to `state +
  question + described options + gold`. Record every source's licence in
  `data/sources.md`.
- **Synthetic rows** produced by a local LLM through any OpenAI-compatible
  endpoint (llama.cpp / Ollama on the local machine), then checked by a second pass.
  Focus areas: email triage, policy-rule matching, "none of these" options,
  and minimal pairs (near-identical inputs with opposite answers).
- **Teacher distributions**: a stronger LLM's option probabilities (through
  logprob readout) used as soft targets, only where the teacher measurably
  beats the student (a Strands v12/v14 lesson).
- **Personal data from Post Office** (section 6.5): LLM decisions as teacher
  labels, and user corrections as gold labels.

### 4.3 Training recipe (lessons from Strands Decider, Von and Laya)
- **Shuffle option order** on every example in every epoch, remapping the
  label. Scores are only reversed, never permuted.
- **Ordinal smoothing**: move about 10% of a score target's mass onto
  adjacent levels.
- **Loss**: listwise cross-entropy over the markers, plus a Brier term for
  calibration, plus optional KL to teacher distributions.
- **Drop rows that don't fit, never truncate them.** At serving time, fit the
  question first, then trim the state from the front, with a visible
  "truncated" flag.
- **Vary the question text** so the model actually reads the question rather
  than memorising per-task priors (Strands v11 failed on this).
- **Balance away shortcuts**: per-task label priors, and position or option
  length cues.
- Checkpoints are saved as safetensors together with config, tokenizer,
  calibration map, a manifest with sha256s, and the training config plus
  data hashes for provenance.

### 4.4 Compute
Training tiers by available memory; `rverdict train` detects memory and picks
the tier, or explains why it cannot:

| Tier | Needs | Notes |
|---|---|---|
| Calibration only | Any machine, CPU is fine | Minutes |
| Head + LoRA fine-tune | A modest GPU or a fast CPU | Small optimiser state |
| Full fine-tune (395M, AdamW) | ≈ 8 GB+ of GPU memory | 6–7 GB for weights and optimiser state, plus activations |

For scale, the reference iGPU ran a full fine-tune at ≈ 570 tokens/s: about
1.2 h per epoch over 5,000 emails, about 54 h per epoch over a Von-sized
public corpus. Large runs can use the same Rust binary on a rented CUDA GPU.

### 4.5 Research discipline
- Every experiment in `research/` states its prediction and failure condition
  **before** the run, and the outcome is appended after it.
- A run is promoted only if it beats the reference on the paired test.
  Differences inside retrain noise count as "unresolved".

---

## 5. Evaluation (`rverdict-eval`)
- Metrics: accuracy, macro accuracy, NLL, Brier, ECE, reliability curve,
  latency (p50/p95), and order-flip rate.
- Suites:
  - **JevBench public** (231 tasks), for comparability with Jev, Von, Laya
    and Strands. Temporary and isolated, see section 9.
  - **jabr classifier-benchmark v2** (out of domain).
  - **Post Office holdout**: a private, frozen set of the user's own labelled
    emails. This is the suite that matters most.
- **Parity checks without Python**: reproduce Von's published JevBench public
  accuracy within noise using its weights, and match laya-rust's outputs on
  Laya weights to about 1e-3.
- `rverdict eval --suite <x> --model <dir> --against <run.json>` prints the
  paired McNemar result.

---

## 6. Post Office integration

### 6.1 Dependency
`post-office-core` gets a `decision` module that depends on `rverdict-core`
and, behind the `embedded` feature, `rverdict-engine`. Model weights are
downloaded on first run (not bundled in the installer): show progress in the
UI, verify sha256 against the manifest, and store them in the app data
directory. The model loads once
at startup (or lazily) and inference runs on a dedicated blocking pool
(`spawn_blocking` or rayon), so the Tokio runtime is never stalled.
`rverdict-remote` is also supported, so TypeSafe's Jev cloud, Cloudflare
Workers AI (Clef, Clef-flash, or Jev as a third-party model) or a separate
server can be used instead. Remote backends are marked non-local for the
privacy routing rules; local stays the default.

### 6.2 Rule → question mapping
| Rule shape | Question |
|---|---|
| instruction only | `noul`: instruction phrased as a statement about the email |
| menu of choices | `choice` with each choice's description, plus an explicit described "none of these apply" option |
| `choose_from_all_labels` | `choice` over label names with descriptions; split into two steps (coarse, then fine) when there are more than about 20 labels |
| instruction + menu | `noul` for whether the rule applies, plus `choice` for which option, in the same pass |

All candidate rules for an email are asked in **one forward pass**. The
existing priority ordering, `continue_after_match` and action resolution
stay in Rust, unchanged.

### 6.3 Decision modes per rule
- `llm` (today's behaviour)
- `verdict`
- `verdict_then_llm`: act when confidence is at or above the threshold,
  otherwise send the email to the existing LLM router.
- `verdict_shadow`: run rverdict alongside the LLM and record both, but act
  only on the LLM. This is the default for the first rollout.

Per-rule confidence thresholds, with a global default of 0.9. Below the threshold
with no LLM fallback, apply a "needs review" label instead of acting.

From Phase 2:
- **Calibrate per rule.** A rule is a fixed question, and calibration fitted
  on one question learns that question's bias; each rule gets its own
  calibration from its own labelled history (shadow-mode LLM labels and user
  corrections), refit as the history grows, and judged by held-out NLL.
- **Cap long emails** with `set_max_state_tokens` (2,048 suggested): attention
  cost grows with the square of the length, and the head and tail of an email
  carry most of what a rule needs.
- **Use f16 on GPUs** once the self-test covers it.

### 6.4 Storage and UI
- Migration: store the decision backend, model id and version, the
  probabilities as JSON, the confidence, and in shadow mode the agreement
  with the LLM.
- Inference Studio: confidence, runner-up, how often rverdict and the LLM
  agree per rule, and estimated latency and cost savings.
- Dry-run and evaluate (`evaluate_messages`): compare rverdict and the LLM
  side by side on real mail.

### 6.5 Feedback loop (the data that drives quality)
- **Implicit feedback**: notice through Gmail history when the user undoes
  one of our actions (removes our label, moves mail out of trash or spam,
  re-archives). That is a negative gold label.
- **Explicit feedback**: thumbs up or down on history entries.
- **Teacher labels**: LLM decisions from shadow mode, weighted below gold.
- **Export**: `post-office export-decisions` writes rverdict JSONL. The
  history table stores no email bodies, so they are re-fetched from Gmail by
  `email_id` at export time, held locally only, and never committed.
- This loop runs rungs 1–3 of the training ladder on personal data and
  produces a personal checkpoint stored in Post Office's data directory.

### 6.6 Writing rules
- The rule chat LLM helps rewrite rule instructions into decision-friendly
  questions: "ask what is true about the email, not about your policy" and
  "options should be descriptive phrases, not single-word tokens".
- The LLM stays responsible for authoring rules, fallback on hard cases, and
  acting as the teacher.

---

## 7. Phases

Each phase lists what it delivers and the criteria for calling it done.
Training work starts in Phase 3 and then runs in parallel with integration,
so it never blocks Post Office.

### Phase 0: technical spikes ✅ done 2026-10-03
- Port ModernBERT to Burn and load `answerdotai/ModernBERT-large`
  safetensors; check hidden states against candle-transformers' ModernBERT
  (Rust against Rust) to about 1e-4.
- Burn backends on this machine: measure inference latency (CPU, Vulkan)
  and training throughput (tokens/sec) for 395M.
- Import Von's `option_marker.pt` and `marker_calibration.json`.
- **Done when**: Burn is confirmed (or replaced by candle) using measured
  numbers, and Von's weights load.
- **Outcome**: Burn confirmed, Vulkan primary, Flex as CPU fallback; Von
  loads and answers with exact order invariance. Loading ModernBERT-large
  itself is carried into Phase 1.

### Phase 1: core + engine with Von parity
- `rverdict-core`, `rverdict-model`, `rverdict-engine` and `rverdict ask`.
- Independent-options masking, multiple questions per pass, truncation
  policy.
- Runtime backend selection (evaluate `burn-dispatch`), startup self-test
  and CPU fallback.
- Tiny-model cross-backend parity tests and the CI matrix (section 3.1a).
- f16 inference on GPUs.
- Load `answerdotai/ModernBERT-large` (carried over from Phase 0).
- **Done when**: Von's JevBench public accuracy is reproduced within noise
  and the order-flip rate is 0.
- **Outcome** ✅ 2026-10-03: 137/231 on the public set, hard tier 43/111 vs
  Von's own 42/111; identical predictions on CPU, Vulkan f32 and Vulkan f16;
  0 order flips. Warm start 3.1 s; f16 opt-in at 2.5× speed. Details in
  `research/phase-1-engine.md`.

### Phase 2: evaluation + calibration
- `rverdict-eval`, `rverdict calibrate`, `rverdict-server`,
  `rverdict-remote`.
- **Done when**: a calibration refit measurably lowers ECE on a held-out
  split.
- **Outcome** ✅ 2026-10-03, with the criterion changed to NLL: on 1,988
  held-out Enron decisions the refit raised zero-shot noul accuracy from
  0.677 to 0.737 and lowered NLL from 0.630 to 0.595 overall (95% interval of
  the change [−0.045, −0.026]). ECE could not show it: it was already 0.011
  overall, because what was wrong was a bias, not overconfidence. NLL, a
  proper scoring rule, is the calibration criterion from now on. The server,
  remote client and `eval compare` shipped; capturing found and fixed two GPU
  correctness bugs (`research/phase-2-calibration.md`).

### Phase 3: training loop
- `rverdict-data` (HF parquet builders, synthetic generator),
  `rverdict-train` (losses, shuffling, smoothing, checkpointing, LoRA).
- **Done when**: fine-tuning from Von on a public task mix improves a
  held-out suite with a significant paired result, end to end in Rust.

### Phase 4: Post Office shadow mode (in parallel with Phase 3)
- Decision module, rule mapping, `verdict_shadow` mode, migration, Inference
  Studio views, feedback capture, decision export.
- **Done when**: two or more weeks of shadow data with agreement and
  calibration reported per rule.

### Phase 5: personal model + rollout
- Calibrate, then LoRA-tune on personal data; switch rules to
  `verdict_then_llm` once their holdout accuracy and confidence bands meet
  the bar.
- **Done when**: most rules run through rverdict, with LLM fallback below
  the threshold.

### Phase 6: own base checkpoint
- Train from ModernBERT-large on the full rverdict corpus, pre-registered,
  and compare against Von, Laya and Jev numbers.
- Optional ModernBERT-base fast tier.

### Later / optional
- Logprob-readout backend over a GGUF LLM via llama.cpp (the jev-bridge
  idea): a no-training backend that uses the AMD GPU through Vulkan.
- Strands-Decider decoder backend (needs a Gated DeltaNet port).
- MCP server so other tools can use rverdict.

---

## 8. Risks and mitigations
| Risk | Mitigation |
|---|---|
| Backends disagree numerically, or a GPU driver is buggy | Startup self-test against CPU with automatic fallback; tiny-model parity tests in CI |
| CUDA and ROCm untested without the hardware | Compile-checked in CI; periodic rented-GPU runs against a checklist; community `rverdict bench` reports |
| Small GPUs run out of memory | f16 inference; training tiers chosen by detected memory |
| ModernBERT port has subtle bugs (RoPE, local/global attention, unpadding) | Compare activations against the candle-transformers implementation layer by layer |
| Open models are weak zero-shot on nuanced rules | Shadow mode, LLM fallback below threshold, personal fine-tuning |
| Calibration doesn't carry over to email | Always refit on the Post Office holdout before trusting thresholds |
| Many-option label menus (>20) | Two-step choice (coarse, then fine); raise the option token budget |
| Prompt injection inside emails | Output is limited to the given options; destructive actions keep their confirmation and threshold gates |
| The field moves weekly | Keep backends behind `DecisionModel`, keep the wire format compatible, and re-benchmark new open weights through `rverdict eval` |
| Privacy of training data | Personal data and checkpoints stay local and are gitignored; export is opt-in |

## 9. Decisions (resolved 2026-10-03)
| Question | Decision |
|---|---|
| Repo | Separate repo `rverdict`; Post Office depends on it as a git dependency |
| Language | English only |
| Python | None at any stage; training is in Rust from the start |
| Own model training | In scope from the start (section 4), but never blocks Post Office rollout |
| JevBench | Use the public set for now, because other open projects do. Keep it isolated so it is easy to remove later: download it at eval time, never commit it, never train on it, and run it as one optional suite in `rverdict-eval`. Follow-up: replace it with an rverdict-owned public suite built from licence-clear sources, then remove the JevBench integration |
| Model weights in Post Office | Download on first run, not bundled in the installer. Show progress, check the sha256 against the manifest, and store them in the app data directory |
| Promotion bar (shadow → `verdict_then_llm`) | On the Post Office holdout, rverdict must be at least as accurate as the LLM, and no more than 5% of its answers given at ≥ 0.9 confidence may be wrong |
| Default act threshold | 0.9 confidence; can be set per rule. Revisit 0.95 once calibration data on real mail exists |
| Remote backends at launch | TypeSafe Jev cloud and Cloudflare Workers AI (Clef, Clef-flash, Jev), both through the Jev-compatible `rverdict-remote` client and marked non-local. Local is the default and the goal |
| Weight downloads | Straight from Hugging Face (`hf-hub` crate), pinned to a revision with sha256 checks |
| Framework and backends | Burn 0.21; wgpu (auto adapter) as the default GPU path, CUDA and ROCm optional, Flex always present as the CPU fallback; chosen at runtime; autotune and fusion off |
| Calibration criterion | Held-out NLL (a proper scoring rule), with ECE reported alongside; per question type, and per rule in Post Office |
| Target hardware | Everyone's: open source, portable across vendors and operating systems. The 890M machine is only the reference for Phase 0 numbers |

## 10. Open questions
- None blocking Phase 1.

## 11. Follow-ups
- Replace JevBench with an rverdict-owned public evaluation suite, then
  remove the JevBench runner and any references to it. The Jev Decision Index
  (38 public benchmarks, `apolinario/decision-index`) is a candidate starting
  point.
- Report the autotune bug upstream to cubecl with the Enron reproduction
  (`research/phase-2-calibration.md`); re-enable autotune once fixed or once
  tuning keys are exact shapes. Measure training throughput without it in
  Phase 3.
- Detect GPU device loss and fail the request instead of returning an answer
  computed on a reset device.
- Call TypeSafe and Cloudflare for real once API keys are available, and add
  Jev's Workers AI model id when Cloudflare documents it.
- CPU speed: Flex's fused attention (~36 GFLOP/s) and broadcast element-wise
  kernels dominate CPU inference. Options: upstream Flex work or an attention
  kernel of our own.
- Make f16 the default on GPUs: extend the self-test to f16 per device.
- Von's zero-shot `noul` leans the wrong way on some questions (0.21 on an
  obvious refund request). Per-rule calibration fixes the bias it shows on a
  given rule (+6 points on Enron spam); training fixes the model. Until a rule
  is calibrated, Post Office should prefer described `choice` options or
  `noul` with explicit criteria.
- Run the lavapipe, Metal and Windows CI jobs on the first push; run CUDA and
  ROCm on real hardware per `docs/backend-checklist.md`.
- Retire `rverdict-spike`'s private copy of Von packing now that the engine
  owns it.
