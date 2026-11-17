// The stylesheet's accessibility numbers, computed from the file itself so a
// colour change can't quietly break contrast.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "styles.css"), "utf8");

/** The `--name: value` declarations of the first block matching `selector`. */
function tokens(block: string): Record<string, string> {
  return Object.fromEntries([...block.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
}
const light = tokens(css.slice(css.indexOf(":root {"), css.indexOf("@media (prefers-color-scheme: dark)")));
const darkStart = css.indexOf("@media (prefers-color-scheme: dark)");
const dark = { ...light, ...tokens(css.slice(darkStart, css.indexOf("\n}\n", darkStart + 200))) };

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255) as [number, number, number];
  const f = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

describe("muted text contrast (U13)", () => {
  // Measured when this was fixed: light #66635d is 5.5 on canvas, 5.2 on
  // sunken, 6.0 on surface; dark #9c988f is 6.5, 5.7 and 6.1. WCAG AA for
  // body text is 4.5. The old light #77746e was 4.3 and 4.05.
  for (const [name, theme] of [["light", light], ["dark", dark]] as const) {
    for (const bg of ["--canvas", "--sunken", "--surface"]) {
      test(`${name}: --muted on ${bg} is at least 4.5:1`, () => {
        expect(contrast(theme["--muted"]!, theme[bg]!)).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});

describe("reading comfort (U25)", () => {
  test("smooth scrolling is off when the person asks for less motion", () => {
    const outside = css.replace(/@media \(prefers-reduced-motion: no-preference\) \{[\s\S]*?\n\}\n/g, "");
    expect(outside).not.toMatch(/scroll-behavior:\s*smooth/);
  });

  test("the base font follows the browser's size setting and is at least 0.875rem", () => {
    const root = css.slice(css.indexOf(":root {"));
    const size = /font-size:\s*([\d.]+)(rem|px|%)/.exec(root.slice(root.indexOf("font-family:")))!;
    expect(size[2]).toBe("rem");
    expect(Number(size[1])).toBeGreaterThanOrEqual(0.875);
  });

  test("no text is smaller than 12px at the base size", () => {
    const small = [...css.matchAll(/font-size:\s*([\d.]+)(rem|px)/g)]
      .map((m) => ({ text: m[0], px: m[2] === "rem" ? Number(m[1]) * 14 : Number(m[1]) }))
      .filter((s) => s.px < 11.9);
    expect(small.map((s) => s.text)).toEqual([]);
  });
});
