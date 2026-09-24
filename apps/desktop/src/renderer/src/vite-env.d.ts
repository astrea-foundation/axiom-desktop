/// <reference types="vite/client" />

import type { DesktopApi } from "../../preload/index";
import type { DemoApi } from "./types";

declare global {
  interface Window {
    axiomDesktop?: DesktopApi;
    __axiomDemo?: DemoApi;
  }
}

export {};
