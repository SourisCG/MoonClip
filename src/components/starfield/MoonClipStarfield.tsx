import { useEffect, useRef } from "react";

interface Star {
  x: number;
  y: number;
  size: number;
  baseAlpha: number;
  speed: number;
  phase: number;
}

const STAR_COUNT = 85;

/** MoonClip starfield: cheap canvas twinkle, fully paused when hidden. */
export function MoonClipStarfield() {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let animationId = 0;
    let isPaused = false;
    let startTime = performance.now();

    const resizeCanvas = () => {
      canvas.width = window.innerWidth;
      canvas.height = window.innerHeight;
    };
    resizeCanvas();
    window.addEventListener("resize", resizeCanvas);

    const stars: Star[] = Array.from({ length: STAR_COUNT }, () => ({
      x: Math.random() * window.innerWidth,
      y: Math.random() * window.innerHeight,
      size: Math.random() * 1.6 + 0.4,
      baseAlpha: Math.random() * 0.5 + 0.2,
      speed: Math.random() * 0.5 + 0.2, // rad/s — slow, independent per star
      phase: Math.random() * Math.PI * 2,
    }));

    const render = () => {
      if (isPaused) return;
      const t = (performance.now() - startTime) / 1000;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      for (const star of stars) {
        // Soft twinkle: small amplitude around base, desynced via phase.
        const a = Math.max(0.05, Math.min(1, star.baseAlpha + Math.sin(t * star.speed + star.phase) * 0.12));
        ctx.fillStyle = `rgba(224, 231, 255, ${a})`;
        ctx.beginPath();
        ctx.arc(star.x, star.y, star.size, 0, Math.PI * 2);
        ctx.fill();
        if (star.size > 1.4 && a > 0.6) {
          ctx.fillStyle = `rgba(56, 189, 248, ${a * 0.25})`;
          ctx.beginPath();
          ctx.arc(star.x, star.y, star.size * 2.5, 0, Math.PI * 2);
          ctx.fill();
        }
      }
      animationId = requestAnimationFrame(render);
    };
    render();

    const pause = () => {
      isPaused = true;
      cancelAnimationFrame(animationId);
    };
    const resume = () => {
      if (!isPaused) return;
      isPaused = false;
      render();
    };
    const onVisibility = () => {
      if (document.hidden) pause();
      else resume();
    };
    const onBlur = () => pause();
    const onFocus = () => {
      // Only resume when the document is actually visible; blur fires
      // on minimize where focus may not return immediately.
      if (!document.hidden) resume();
    };

    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);

    return () => {
      isPaused = true;
      cancelAnimationFrame(animationId);
      window.removeEventListener("resize", resizeCanvas);
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden
      className="pointer-events-none fixed inset-0 z-0 opacity-70"
    />
  );
}
