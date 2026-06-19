import { Routes, Route } from "react-router-dom";
import Dashboard from "./pages/Dashboard";
import AccountDetail from "./pages/AccountDetail";
import AccountsConfig from "./pages/AccountsConfig";
import Settings from "./pages/Settings";

function App() {
  return (
    <div className="app">
      <Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/account/:id" element={<AccountDetail />} />
        <Route path="/config" element={<AccountsConfig />} />
        <Route path="/settings" element={<Settings />} />
      </Routes>
    </div>
  );
}

export default App;
