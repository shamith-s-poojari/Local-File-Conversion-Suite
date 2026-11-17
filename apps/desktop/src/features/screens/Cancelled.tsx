import { CloseIcon } from "../../icons";
import type { Send } from "./shared";

export function Cancelled({ send }: { send: Send }) {
  return (
    <section className="screen narrow state-screen" aria-labelledby="cancelled-title">
      <span className="state-icon">
        <CloseIcon />
      </span>
      <h1 id="cancelled-title" tabIndex={-1}>Cancelled</h1>
      <p className="lede">Nothing was saved, and the job's temporary files were deleted.</p>
      <div className="actions center">
        <button type="button" className="primary" onClick={() => send({ type: "EDIT" })}>
          Back to settings
        </button>
      </div>
    </section>
  );
}

