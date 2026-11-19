// Shared test fixtures: files, contexts and diagnostics shaped like the real thing.

import type { Api, Diagnostics, FileDescriptor } from "./api";
import type { Context, FileView } from "./machines/app";

export function descriptor(id: string, extra: Partial<FileDescriptor> = {}): FileDescriptor {
  return {
    fileId: id,
    displayName: `${id}.pdf`,
    sizeBytes: 2048,
    format: "pdf",
    confidence: "certain",
    evidence: [],
    extensionMismatch: false,
    candidates: [],
    encryptionHint: "none",
    macroEnabled: false,
    cloudPlaceholder: false,
    actionable: true,
    ...extra,
  };
}

export function view(id: string, extra: Partial<FileView> = {}): FileView {
  return { file: descriptor(id), displayName: `${id}.pdf`, ...extra };
}

export function context(extra: Partial<Context> = {}): Context {
  return {
    files: [view("a")],
    operation: null,
    to: null,
    params: {},
    showBestEffort: false,
    job: null,
    error: null,
    save: null,
    lastSeq: 0,
    notice: null,
    skipped: 0,
    ...extra,
  };
}

/** An `Api` whose commands the test didn't expect to be called. */
export const noApi = {} as Api;

export const diagnostics: Diagnostics = {
  appVersion: "0.1.0",
  protocolVersion: 1,
  adapters: ["pdf-merge", "image-convert"],
  processingEnabled: true,
  engines: ["ffmpeg", "pdf2docx"],
  platform: "macos",
  sandbox: {
    passed: false,
    attempts: [
      { name: "read_sentinel", expected: "blocked", outcome: "blocked", passed: true },
      { name: "connect_external", expected: "blocked", outcome: "allowed", passed: false },
    ],
  },
};
