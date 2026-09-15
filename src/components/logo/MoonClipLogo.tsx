import "./MoonClipLogo.css";

interface Props {
  size?: number;
}

/** MoonClip brand mark: red→blue crescent moon + white play (user's own logo). */
export function MoonClipLogo({ size = 28 }: Props) {
  const playScale = size / 28;
  return (
    <span
      className="moonclip-logo moonclip-logo-hover"
      aria-hidden
      style={{ width: size, height: size }}
    >
      <span
        className="moonclip-logo-play"
        style={{
          borderTopWidth: 4.5 * playScale,
          borderBottomWidth: 4.5 * playScale,
          borderLeftWidth: 7 * playScale,
        }}
      />
    </span>
  );
}
