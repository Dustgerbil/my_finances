import { SyncProvider } from "./sync";
import React from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router";
import App from "./App";
import "./App.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <HashRouter>
      <SyncProvider><App /></SyncProvider>
    </HashRouter>
  </React.StrictMode>,
);
