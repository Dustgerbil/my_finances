import { Component, lazy, Suspense, type ErrorInfo, type ReactNode } from "react";
import { Routes, Route } from "react-router";

const Dashboard = lazy(() => import("./pages/Dashboard"));
const OtherAccounts = lazy(() => import("./pages/OtherAccounts"));
const AccountDetail = lazy(() => import("./pages/AccountDetail"));
const AccountsConfig = lazy(() => import("./pages/AccountsConfig"));
const Settings = lazy(() => import("./pages/Settings"));

interface AppErrorBoundaryProps {
  children: ReactNode;
}

interface AppErrorBoundaryState {
  error: Error | null;
}

class AppErrorBoundary extends Component<
  AppErrorBoundaryProps,
  AppErrorBoundaryState
> {
  state: AppErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): AppErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("My Finances failed to render", error, info.componentStack);
  }

  private returnToDashboard = () => {
    window.location.hash = "#/";
    window.location.reload();
  };

  render() {
    if (this.state.error) {
      return (
        <div className="empty-state">
          <h2>Something went wrong</h2>
          <p>{this.state.error.message}</p>
          <button className="btn btn-primary" onClick={this.returnToDashboard}>
            Back to Dashboard
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}

function App() {
  return (
    <div className="app">
      <AppErrorBoundary>
        <Suspense fallback={<div className="empty-state"><p>Loading…</p></div>}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/other-accounts" element={<OtherAccounts />} />
            <Route path="/account/:id" element={<AccountDetail />} />
            <Route path="/config" element={<AccountsConfig />} />
            <Route path="/settings" element={<Settings />} />
          </Routes>
        </Suspense>
      </AppErrorBoundary>
    </div>
  );
}

export default App;
