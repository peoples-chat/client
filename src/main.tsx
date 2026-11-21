import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/albert-sans";
import "./styles/tokens.css";
import "./styles/global.css";
import App from "./App";
import { store } from "./state";
import { followVisualViewport } from "./viewport";

followVisualViewport();
void store.start();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
