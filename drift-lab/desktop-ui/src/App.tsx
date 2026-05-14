import { useEffect, useState } from "react";
import { Route, Routes } from "react-router-dom";

import Onboarding from "./components/Onboarding";
import { AppConfig, getAppConfig } from "./lib/tauri";
import HomePage from "./pages/Home";
import ReportPage from "./pages/Report";
import ScanReportPage from "./pages/ScanReport";
import SettingsPage from "./pages/Settings";

export default function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);

  useEffect(() => {
    getAppConfig().then(setConfig);
  }, []);

  if (config === null) {
    return null; // brief bootstrap
  }

  if (!config.onboardingComplete) {
    return <Onboarding onComplete={() => getAppConfig().then(setConfig)} />;
  }

  return (
    <Routes>
      <Route path="/" element={<HomePage />} />
      <Route path="/scan/:scanId" element={<ScanReportPage />} />
      <Route path="/report/:runId" element={<ReportPage />} />
      <Route path="/settings" element={<SettingsPage />} />
    </Routes>
  );
}
