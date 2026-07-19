import { Outlet, NavLink, useNavigate } from "react-router-dom";
import ThemeToggle from "./ThemeToggle";
import ConnectionStatus from "./ConnectionStatus";
import { useGate } from "../lib/gate";

const navItems = [
  { to: "/", label: "Dashboard" },
  { to: "/rules", label: "Rules" },
  { to: "/history", label: "History" },
  { to: "/settings", label: "Settings" },
];

export default function Layout() {
  const { connection } = useGate();
  const navigate = useNavigate();

  return (
    <div className="flex h-screen bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100 transition-colors">
      <nav className="w-56 bg-gray-100 dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700 flex flex-col">
        <div className="p-4 border-b border-gray-200 dark:border-gray-700">
          <h1 className="text-lg font-semibold" data-tauri-drag-region>
            Post Office
          </h1>
        </div>
        <div className="flex-1 p-2">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                `block px-3 py-2 rounded mb-1 text-sm transition-colors ${
                  isActive
                    ? "bg-gray-200 dark:bg-gray-700 text-gray-900 dark:text-white"
                    : "text-gray-500 dark:text-gray-400 hover:bg-gray-200/50 dark:hover:bg-gray-700/50 hover:text-gray-900 dark:hover:text-white"
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}
        </div>
        <div className="p-3 border-t border-gray-200 dark:border-gray-700 space-y-3">
          <button
            onClick={() => navigate("/settings")}
            className="w-full text-left"
            title="Open Settings to connect or manage Gmail"
          >
            <ConnectionStatus connection={connection} />
          </button>
          <ThemeToggle />
        </div>
      </nav>
      <main className="flex-1 overflow-auto p-6">
        <Outlet />
      </main>
    </div>
  );
}
