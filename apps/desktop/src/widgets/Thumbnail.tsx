import { useEffect, useRef, useState } from "react";
import { previewUrl, type Api } from "../api";

/** A page preview loaded only once it scrolls into view, so documents with
 * thousands of pages don't render every page up front. */
export function Thumbnail({
  api,
  reference,
  page,
  dpi = 48,
  rotate = 0,
  alt,
}: {
  api: Api;
  reference: string;
  page: number;
  dpi?: number;
  rotate?: number;
  alt: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        api
          .requestPreview(reference, page, dpi)
          .then((p) => !cancelled && setSrc(previewUrl(p.path)))
          .catch(() => !cancelled && setFailed(true));
      }
    });
    io.observe(el);
    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, [api, reference, page, dpi]);
  return (
    <div ref={ref} className={`thumb rot-${rotate}`}>
      {src ? <img src={src} alt={alt} draggable={false} /> : <span className="thumb-placeholder">{failed ? "!" : page}</span>}
    </div>
  );
}
