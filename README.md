# Post Office

A desktop app (Tauri + Rust) that applies AI-powered filters over your Gmail —
labeling, archiving, trashing, or marking mail as spam based on rules you
define and a local OpenAI-compatible LLM endpoint.

## How Gmail authentication works

Post Office connects to Gmail using **OAuth 2.0**. The important thing to
understand is the split between the *app's* identity and *your* authorization:

- **`client_id`** identifies **the app** (Post Office), not you. It is baked
  into the application, just like every other app that offers "Sign in with
  Google".
- **Your account** is authorized through the normal Google consent screen
  ("Post Office wants to read/modify your mail — allow?").

So you never paste credentials into the app. You just click **Connect Gmail**
and approve the requested permissions.

### PKCE + client secret

Post Office uses the **PKCE** flow (RFC 7636), Google's recommended approach for
native/desktop apps. Note that Google **also requires the `client_secret` even
for Desktop clients** — it validates the value at the token exchange, so a
placeholder won't work. For an installed app the secret is not confidential
(Google's own docs say it "is obviously not treated as a secret"); it is safe to
embed alongside the `client_id`. PKCE is what actually secures the flow.

### The built-in client

The app's `client_id` and `client_secret` live in `src-tauri/google-oauth.json`
(git-ignored) and are **baked into the binary at compile time** (via
`crates/core/build.rs`) and also bundled as a resource. This means the app uses
your credentials by default — **you do not need to set any environment
variable**. A committed `src-tauri/google-oauth.example.json` shows the expected
shape. Both fields are required; PKCE still supplies the `code_verifier`.

To use a *different* client than the baked-in one, either:

1. Set the environment variable before launching (overrides the baked-in id):

   ```sh
   POST_OFFICE_GOOGLE_CLIENT_ID=1234567890-abc.apps.googleusercontent.com cargo tauri dev
   ```

2. Or change `src-tauri/google-oauth.json` (rebuild required) — it is read at
   both build time (baked) and, if present, at runtime.

Resolution order is: env var → bundled `google-oauth.json` → baked-in
`client_id` → `google.client_id` in app config.

### One-time Google Cloud setup (for the app's client)

If you create your **own** OAuth client (instead of reusing the bundled one):

1. In Google Cloud Console, create an **OAuth client ID** of type **Desktop
   app**.
2. Add `http://localhost:7890` as an authorized redirect URI.
3. On the OAuth consent screen, add **your own Google account** as a **test
   user**. This is required because `gmail.modify` is a *sensitive* scope — for
   personal use, being a test user lets you skip Google's verification review.

The callback listener runs on `http://localhost:7890` only for the duration of
the authorization and exchanges the code for tokens, which are stored in your
OS keyring.

## Running

```sh
npm install
cargo tauri dev      # development
cargo tauri build    # release bundles (.deb / .rpm / etc.)
```

First launch shows an onboarding screen. Connect Gmail, configure your LLM
endpoint (e.g. a local Ollama instance at `http://localhost:11434/v1`), then
create a rule.

## License

MIT
