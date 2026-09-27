import { useEffect } from "react";
import { createPortal } from "react-dom";
import type { ReactNode } from "react";

import { pushOverlay } from "../lib/overlay";

/**
 * Render modal overlays into `document.body`.
 *
 * The app shell's `main` is scrollable and has `backdrop-blur-xl`, and a
 * `backdrop-filter` ancestor creates a containing block for `fixed`
 * descendants: `fixed inset-0` panels were positioned against `main` and
 * scrolled away with the gallery. Portaling to the body keeps them truly
 * viewport-fixed (centered, header always visible) no matter the scroll.
 */
export function Modal({ children }: { children: ReactNode }) {
  useEffect(() => pushOverlay(), []);
  return createPortal(children, document.body);
}
