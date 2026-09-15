import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import "./i18n";

declare const __MOONCLIP_BUILD__: string | undefined;
try {
  console.debug(
    `[moonclip] frontend build: ${typeof __MOONCLIP_BUILD__ !== "undefined" ? __MOONCLIP_BUILD__ : "dev"}`,
  );
} catch {
  /* noop */
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
