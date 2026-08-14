import "@testing-library/jest-dom/vitest";

import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

declare global {
  // React reads this off the global object; it ships no type for it.
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

// React only honours `act()` when told it is running under a test renderer.
// Without this, a component updated from outside an event handler — a Tauri
// listener, say — warns and can be asserted on before it has re-rendered.
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  cleanup();
});
