import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { App } from "./App";

test("home screen states that processing is local", () => {
  expect(renderToStaticMarkup(<App />)).toContain("Processed on your device");
});
