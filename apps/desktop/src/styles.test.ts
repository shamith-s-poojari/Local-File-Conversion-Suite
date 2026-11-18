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

describe("control borders (A11Y: WCAG 1.4.11)", () => {
  // The old translucent --line-strong was 1.44:1; a control's boundary needs 3:1.
  for (const [name, theme] of [["light", light], ["dark", dark]] as const) {
    for (const bg of ["--canvas", "--sunken", "--surface"]) {
      test(`${name}: --line-strong on ${bg} is at least 3:1`, () => {
        expect(theme["--line-strong"]).toMatch(/^#[0-9a-f]{6}$/i);
        expect(contrast(theme["--line-strong"]!, theme[bg]!)).toBeGreaterThanOrEqual(3);
      });
    }
  }
});

describe("text on its tinted background (A11Y: WCAG 1.4.3)", () => {
  const pairs = [
    ["--doc-ink", "--doc-bg"],
    ["--img-ink", "--img-bg"],
    ["--aud-ink", "--aud-bg"],
    ["--vid-ink", "--vid-bg"],
    ["--dat-ink", "--dat-bg"],
    ["--error", "--error-bg"],
    ["--warn", "--warn-bg"],
    ["--ok", "--ok-bg"],
  ];
  for (const [fg, bg] of pairs) {
    test(`light: ${fg} on ${bg} is at least 4.5:1`, () => {
      expect(contrast(light[fg!]!, light[bg!]!)).toBeGreaterThanOrEqual(4.5);
    });
  }
  test("light: the focus colour, used for links, reads on every background", () => {
    for (const bg of ["--canvas", "--sunken", "--surface"]) expect(contrast(light["--focus"]!, light[bg]!)).toBeGreaterThanOrEqual(4.5);
  });
});

describe("motion and transparency preferences (A11Y-1)", () => {
  const reduce = css.slice(css.indexOf("@media (prefers-reduced-motion: reduce)"));
  const block = reduce.slice(0, reduce.indexOf("\n}\n"));

  test("under reduced motion nothing loops: animations run once", () => {
    expect(block).toMatch(/animation-iteration-count:\s*1\s*!important/);
    expect(block).toContain("animation-duration: 1ms");
  });

  test("the spinner and skeleton stop, and the moving bar becomes a still stripe", () => {
    expect(block).toMatch(/\.state-icon\.spinning,\s*\.skeleton\s*\{\s*animation:\s*none/);
    expect(block).toMatch(/\.bar\.indeterminate::after\s*\{[^}]*animation:\s*none[^}]*width:\s*100%/);
  });

  test("blur and see-through bars are solid when transparency is reduced", () => {
    const t = css.slice(css.indexOf("@media (prefers-reduced-transparency: reduce)"));
    const b = t.slice(0, t.indexOf("\n}\n"));
    expect(b).toContain("backdrop-filter: none");
    expect(b).toContain(".action-bar");
  });

  test("a disabled-looking button that keeps focus is styled like a disabled one", () => {
    expect(css).toMatch(/button\[aria-disabled="true"\]\s*\{\s*opacity: 0\.45/);
  });
});
