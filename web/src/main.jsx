import { createRoot } from "react-dom/client";
import App from "./App.jsx";
import "./index.css";

// No StrictMode on purpose: its dev-only double-invoke can double-init the R3F
// canvas / controls and show up as flicker. Re-enable once stable if you want it.
createRoot(document.getElementById("root")).render(<App />);
