import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import DesktopEntry from "./DesktopEntry";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <DesktopEntry />
  </StrictMode>,
);
