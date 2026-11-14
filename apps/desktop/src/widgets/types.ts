import type { Api, JsonSchema } from "../api";
import type { FileView } from "../machines/app";

/** What a custom widget (`x-ui`, spec 12.3) gets to work with. */
export interface WidgetContext {
  files: FileView[];
  /** Pages in the first selected file, from `inspect`. */
  pages: number;
  /** `fileId` of the first selected file, for previews and page text. */
  reference: string;
  api: Api;
}

export interface WidgetProps<T> {
  id: string;
  label: string;
  schema: JsonSchema;
  value: T | undefined;
  onChange: (value: T | undefined) => void;
  ctx: WidgetContext;
  /** Every param, for widgets that own more than one (the redaction canvas
   * also edits `terms`). */
  params: Record<string, unknown>;
  setParam: (key: string, value: unknown) => void;
}
