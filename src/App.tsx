import { lazy, Suspense } from "react";
import { Routes, Route } from "react-router";

const Dashboard = lazy(() => import("./pages/Dashboard"));
const AccountDetail = lazy(() => import("./pages/AccountDetail"));
const AccountsConfig = lazy(() => import("./pages/AccountsConfig"));
const Settings = lazy(() => import("./pages/Settings"));

function App() {
  return (
    <div className="app">
      <Suspense fallback={<div className="empty-state"><p>Loading…</p></div>}>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/account/:id" element={<AccountDetail />} />
          <Route path="/config" element={<AccountsConfig />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </Suspense>
    </div>
  );
}

export default App;
