import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";
import type { ConversionTarget } from "../api";
import { nextRadio, RadioGroup, tabStop } from "./radio";
import { TargetTile } from "./screens/Configure";

const item = (checked = false, disabled = false) => ({ checked, disabled });

describe("which radio is the tab stop (U10)", () => {
  test("the checked one", () => {
    expect(tabStop([item(), item(true), item()])).toBe(1);
  });
  test("with none checked, the first one that isn't disabled", () => {
    expect(tabStop([item(false, true), item(), item()])).toBe(1);
  });
  test("a checked one that is disabled doesn't hold the tab stop", () => {
    expect(tabStop([item(), item(true, true)])).toBe(0);
  });
  test("none when everything is disabled", () => {
    expect(tabStop([item(false, true)])).toBe(-1);
  });
});

describe("arrow keys move between radios (U10)", () => {
  const items = [item(), item(false, true), item(), item()];
  test("forward and down go to the next enabled one, wrapping", () => {
    expect(nextRadio("ArrowRight", 0, items)).toBe(2);
    expect(nextRadio("ArrowDown", 3, items)).toBe(0);
  });
  test("back and up go to the previous enabled one, wrapping", () => {
    expect(nextRadio("ArrowLeft", 2, items)).toBe(0);
    expect(nextRadio("ArrowUp", 0, items)).toBe(3);
  });
  test("Home and End go to the first and last enabled one", () => {
    expect(nextRadio("Home", 3, items)).toBe(0);
    expect(nextRadio("End", 0, items)).toBe(3);
  });
  test("other keys do nothing, and so does a group with one choice", () => {
    expect(nextRadio("a", 0, items)).toBeNull();
    expect(nextRadio("ArrowRight", 0, [item()])).toBeNull();
  });
});

describe("RadioGroup", () => {
  const render = (checked: number) =>
    renderToStaticMarkup(
      <RadioGroup
        label="Tools"
        items={[0, 1, 2].map((i) => ({ key: String(i), checked: i === checked }))}
        render={(it, tabIndex) => (
          <button key={it.key} type="button" role="radio" aria-checked={it.checked} tabIndex={tabIndex}>
            {it.key}
          </button>
        )}
      />,
    );

  test("is a labelled radio group with one tab stop", () => {
    const m = render(1);
    expect(m).toContain('role="radiogroup"');
    expect(m).toContain('aria-label="Tools"');
    expect((m.match(/tabindex="0"/g) ?? []).length).toBe(1);
    expect((m.match(/tabindex="-1"/g) ?? []).length).toBe(2);
    expect(m).toMatch(/aria-checked="true" tabindex="0">1</);
  });
});

describe("TargetTile", () => {
  const c = { to: "png", tier: "E", bestEffort: false, exactWithTextLayer: false } as ConversionTarget;
  test("is a radio that follows the tab stop it is given", () => {
    const stop = renderToStaticMarkup(<TargetTile c={c} checked={false} tabIndex={0} onChoose={() => {}} />);
    const other = renderToStaticMarkup(<TargetTile c={c} checked={false} tabIndex={-1} onChoose={() => {}} />);
    expect(stop).toContain('role="radio"');
    expect(stop).toContain('tabindex="0"');
    expect(other).toContain('tabindex="-1"');
  });
});
