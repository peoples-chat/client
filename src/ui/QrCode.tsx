import { useMemo } from "react";
import { encode } from "uqr";

interface QrCodeProps {
  value: string;
  size: number;
  /** Accessible name of the image. */
  label: string;
}

/** A QR code drawn as one SVG path in the current text colour. The caller adds the quiet zone. */
export function QrCode({ value, size, label }: QrCodeProps) {
  const { path, modules } = useMemo(() => {
    const { data } = encode(value, { ecc: "M", border: 0 });
    let d = "";
    data.forEach((row, y) => {
      // One rectangle per run of dark modules keeps the path short.
      for (let x = 0; x < row.length; x++) {
        if (!row[x]) continue;
        let run = 1;
        while (row[x + run]) run++;
        d += `M${x} ${y}h${run}v1h-${run}z`;
        x += run - 1;
      }
    });
    return { path: d, modules: data.length };
  }, [value]);

  return (
    <svg role="img" aria-label={label} width={size} height={size} viewBox={`0 0 ${modules} ${modules}`} shapeRendering="crispEdges">
      <path d={path} fill="currentColor" />
    </svg>
  );
}
