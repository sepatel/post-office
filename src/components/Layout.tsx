import { Outlet, NavLink } from "react-router-dom";

const navItems = [
  { to: "/", label: "Dashboard" },
  { to: "/rules", label: "Rules" },
  { to: "/history", label: "History" },
  { to: "/settings", label: "Settings" },
];

export default function Layout() {
  return (
    <div className="flex h-screen bg-gray-900 text-gray-100">
      <nav className="w-56 bg-gray-800 border-r border-gray-700 flex flex-col">
        <div className="p-4 border-b border-gray-700">
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
                `block px-3 py-2 rounded mb-1 text-sm ${
                  isActive
                    ? "bg-gray-700 text-white"
                    : "text-gray-400 hover:bg-gray-700/50 hover:text-white"
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}
        </div>
      </nav>
      <main className="flex-1 overflow-auto p-6">
        <Outlet />
      </main>
    </div>
  );
}
