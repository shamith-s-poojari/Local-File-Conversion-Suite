import { useEffect, useRef, useState, type ReactNode } from "react";
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
  fallback,
}: {
  api: Api;
  reference: string;
  page: number;
  dpi?: number;
  rotate?: number;
  alt: string;
  /** Shown instead of "No preview" when the preview fails. */
  fallback?: ReactNode;
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
  if (failed && fallback) return <>{fallback}</>;
  return (
    <div ref={ref} className={`thumb rot-${rotate}${src ? " loaded" : ""}`}>
      {src ? (
        <img src={src} alt={alt} draggable={false} />
      ) : (
        <span className="thumb-placeholder">{failed ? "No preview" : <span className="skeleton" />}</span>
      )}
    </div>
  );
}
