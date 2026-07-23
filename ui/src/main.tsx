import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { QuickCaptureWindow } from "./features/quick-capture";
import { currentWindowLabel } from "./lib/api";
import "./index.css";

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("Casual Note: #root element not found");
}

// Both windows load the same bundle (tauri.conf.json). Branch on the window label
// so the frameless "quick-capture" window renders its own minimal surface.
const isQuickCapture = currentWindowLabel() === "quick-capture";
if (isQuickCapture) {
  document.body.classList.add("quick-capture-body");
}

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>{isQuickCapture ? <QuickCaptureWindow /> : <App />}</React.StrictMode>,
);
