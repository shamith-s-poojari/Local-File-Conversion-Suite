// What the UI says about each format: its name, family and a short line.
// Families group the "Convert to" choices and colour their tags.

import type { Format, MediaInfo, Tier } from "./api";

export type Family = "document" | "data" | "image" | "audio" | "video";

export interface FormatInfo {
  label: string;
  family: Family;
  /** Shown under the label in the target picker. */
  note: string;
}

export const FORMATS: Record<Exclude<Format, "unknown">, FormatInfo> = {
  pdf: { label: "PDF", family: "document", note: "Fixed layout, opens anywhere" },
  docx: { label: "Word", family: "document", note: "DOCX, editable" },
  doc: { label: "Word 97", family: "document", note: "DOC, older Word" },
  odt: { label: "OpenDocument text", family: "document", note: "ODT, LibreOffice" },
  rtf: { label: "Rich text", family: "document", note: "RTF, simple formatting" },
  pptx: { label: "PowerPoint", family: "document", note: "PPTX slides" },
  ppt: { label: "PowerPoint 97", family: "document", note: "PPT, older PowerPoint" },
  odp: { label: "OpenDocument slides", family: "document", note: "ODP, LibreOffice" },
  xlsx: { label: "Excel", family: "data", note: "XLSX spreadsheet" },
  xls: { label: "Excel 97", family: "data", note: "XLS, older Excel" },
  ods: { label: "OpenDocument sheet", family: "data", note: "ODS, LibreOffice" },
  csv: { label: "CSV", family: "data", note: "Comma-separated values" },
  md: { label: "Markdown", family: "document", note: "Plain text with formatting" },
  html: { label: "HTML", family: "document", note: "Web page" },
  txt: { label: "Plain text", family: "document", note: "TXT, words only" },
  epub: { label: "EPUB", family: "document", note: "E-book" },
  json: { label: "JSON", family: "data", note: "Structured data" },
  yaml: { label: "YAML", family: "data", note: "Readable structured data" },
  png: { label: "PNG", family: "image", note: "Lossless, keeps transparency" },
  jpg: { label: "JPG", family: "image", note: "Photos, small files" },
  webp: { label: "WebP", family: "image", note: "Small files for the web" },
  svg: { label: "SVG", family: "image", note: "Vector drawing" },
  heic: { label: "HEIC", family: "image", note: "iPhone photos" },
  gif: { label: "GIF", family: "image", note: "Animation, 256 colours" },
  bmp: { label: "BMP", family: "image", note: "Uncompressed bitmap" },
  tiff: { label: "TIFF", family: "image", note: "Print and archive" },
  avif: { label: "AVIF", family: "image", note: "Smallest photo files" },
  ico: { label: "ICO", family: "image", note: "Windows icon" },
  mp3: { label: "MP3", family: "audio", note: "Plays everywhere" },
  wav: { label: "WAV", family: "audio", note: "Uncompressed" },
  flac: { label: "FLAC", family: "audio", note: "Lossless, compressed" },
  ogg: { label: "OGG", family: "audio", note: "Vorbis, open format" },
  opus: { label: "Opus", family: "audio", note: "Best quality per size" },
  m4a: { label: "M4A", family: "audio", note: "AAC, Apple devices" },
  aac: { label: "AAC", family: "audio", note: "Raw AAC stream" },
  aiff: { label: "AIFF", family: "audio", note: "Uncompressed, Mac" },
  mp4: { label: "MP4", family: "video", note: "H.264, plays everywhere" },
  mov: { label: "MOV", family: "video", note: "QuickTime" },
  mkv: { label: "MKV", family: "video", note: "Matroska container" },
  webm: { label: "WebM", family: "video", note: "VP9, for the web" },
  avi: { label: "AVI", family: "video", note: "Older Windows video" },
};

export const FAMILIES: { id: Family; label: string; blurb: string }[] = [
  { id: "document", label: "Documents", blurb: "PDF, Word, PowerPoint, OpenDocument, Markdown, HTML, EPUB" },
  { id: "image", label: "Images", blurb: "JPG, PNG, WebP, HEIC, GIF, TIFF, AVIF, SVG, BMP, ICO" },
  { id: "audio", label: "Audio", blurb: "MP3, WAV, FLAC, OGG, Opus, M4A, AAC, AIFF" },
  { id: "video", label: "Video", blurb: "MP4, MOV, MKV, WebM, AVI, and GIF from video" },
  { id: "data", label: "Spreadsheets and data", blurb: "Excel, OpenDocument, CSV, JSON, YAML" },
];

export function formatInfo(f: Format): FormatInfo {
  return f === "unknown" ? { label: "Unknown", family: "document", note: "" } : FORMATS[f];
}

export function familyOf(f: Format): Family {
  return formatInfo(f).family;
}

/** What a tier means for the person converting. */
export const TIER_TEXT: Record<Tier, { label: string; help: string }> = {
  E: { label: "Lossless", help: "Nothing is lost." },
  H: { label: "High quality", help: "Looks the same; some inner details may change." },
  R: { label: "Best effort", help: "Rebuilt from the layout. Check the result." },
  L: { label: "Smaller file", help: "Some quality is traded for a smaller file." },
};

export function formatDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** "1920 × 1080 · 2:14 · H.264" */
export function describeMedia(m: MediaInfo | undefined): string {
  if (!m) return "";
  const parts: string[] = [];
  if (m.width && m.height) parts.push(`${m.width} × ${m.height}`);
  if (m.durationMs) parts.push(formatDuration(m.durationMs));
  if (m.kind === "image" && m.frames && m.frames > 1) parts.push(`${m.frames} frames`);
  if (m.kind === "audio") {
    if (m.sampleRate) parts.push(`${(m.sampleRate / 1000).toFixed(1).replace(/\.0$/, "")} kHz`);
    if (m.channels) parts.push(m.channels === 1 ? "mono" : m.channels === 2 ? "stereo" : `${m.channels} ch`);
  }
  if (m.kind === "video" && m.hasAudio === false) parts.push("no sound");
  return parts.join(" · ");
}
