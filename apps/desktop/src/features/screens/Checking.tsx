import { CHECKING } from "../../text";
import type { Send } from "./shared";

/** While each file is opened in its sandbox. It can be left (audit UX-7). */
export function Checking({ send }: { send: Send }) {
  return (
    <section className="screen narrow state-screen" aria-labelledby="checking-title" aria-busy="true">
      <span className="state-icon spinning" aria-hidden="true" />
      <h1 id="checking-title" tabIndex={-1}>
        {CHECKING.title}
      </h1>
      <p className="lede">{CHECKING.lede}</p>
      <div className="actions center">
        <button type="button" onClick={() => send({ type: "CANCEL" })}>
          {CHECKING.cancel}
        </button>
      </div>
    </section>
  );
}
