/**
 * Global "an overlay is open" signal.
 *
 * Modals are portaled to `document.body`; while one is on screen the
 * starfield behind the blurred shell keeps repainting (and re-blurring) the
 * whole window every frame, which is very expensive in software compositing.
 * The starfield subscribes here and pauses while any modal is open.
 */
type Listener = (open: boolean) => void;

let count = 0;
const listeners = new Set<Listener>();

/** Register an overlay; call the returned function when it unmounts. */
export function pushOverlay(): () => void {
  count += 1;
  listeners.forEach((listener) => listener(true));
  return () => {
    count = Math.max(0, count - 1);
    listeners.forEach((listener) => listener(count > 0));
  };
}

export function onOverlayChange(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
