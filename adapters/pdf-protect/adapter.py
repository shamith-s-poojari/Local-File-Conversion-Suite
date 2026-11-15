"""pdf-protect: add AES-256 protection, or remove it with the password (spec section 13.6).

There is no guessing, retry automation or cracking. Unlocking a file with an
open password needs that password; unlocking a permissions-only file needs
the user to confirm they have the right to.
"""

from __future__ import annotations

import secrets as random_secrets
from pathlib import Path
from typing import Any, ClassVar, Literal

import pikepdf
from converter_sdk import AdapterError, JobContext, OutputFile, run_main
from converter_sdk.pdf import OpenedInput, PdfAdapter, encryption_state, param_issue, save_pdf
from converter_sdk.protocol import ValidationIssue

SECRET_PARAMS = ("userPassword", "ownerPassword", "password")


def permissions(params: dict[str, Any]) -> pikepdf.Permissions:
    p = params.get("permissions") or {}
    allow_print = bool(p.get("print", True))
    allow_modify = bool(p.get("modify", True))
    allow_annotate = bool(p.get("annotate", True))
    return pikepdf.Permissions(
        accessibility=True,  # screen readers must keep working
        extract=bool(p.get("copy", True)),
        modify_annotation=allow_annotate,
        modify_form=allow_annotate,
        modify_assembly=allow_modify,
        modify_other=allow_modify,
        print_lowres=allow_print,
        print_highres=allow_print,
    )


class PdfProtectAdapter(PdfAdapter):
    adapter_id: ClassVar[str] = "pdf-protect"
    fidelity: ClassVar[Literal["E", "H", "R", "L"]] = "E"

    def validate_params(
        self, ctx: JobContext, params: dict[str, Any], inputs: list[OpenedInput]
    ) -> list[ValidationIssue]:
        if any(k in params for k in SECRET_PARAMS):
            # Params can be logged and stored; passwords may only arrive as secrets.
            return [param_issue("secrets", "passwordInParams")]
        if len(inputs) != 1:
            return [param_issue("inputs", "exactlyOne")]
        pdf = inputs[0].pdf
        mode = params.get("mode", "protect")
        if mode == "protect":
            if pdf.is_encrypted:
                return [param_issue("mode", "alreadyEncrypted")]
            if ctx.secrets is None or ctx.secrets.userPassword is None:
                return [param_issue("userPassword", "required")]
            return []
        if mode == "unlock":
            if not pdf.is_encrypted:
                return [param_issue("mode", "notEncrypted")]
            if self._needs_confirmation(ctx, inputs[0].path, pdf) and (
                params.get("confirmRights") is not True
            ):
                return [param_issue("confirmRights", "required")]
            return []
        return [param_issue("mode", "invalid")]

    @staticmethod
    def _needs_confirmation(ctx: JobContext, path: Path, pdf: pikepdf.Pdf) -> bool:
        """Permissions-only files open without a password, so removing their
        restrictions needs the user's confirmation, unless they proved the
        right by giving the owner password."""
        return encryption_state(ctx, path) == "permissionsOnly" and not pdf.owner_password_matched

    def execute_conversion(self, ctx: JobContext, params: dict[str, Any]) -> list[OutputFile]:
        if any(k in params for k in SECRET_PARAMS):
            raise AdapterError("E_INTERNAL", {"reason": "passwordInParams"}, retryable=False)
        mode = params.get("mode", "protect")
        ctx.stage(mode, 1, 1)
        with ctx.open_pdf(ctx.inputs[0]) as pdf:
            self.warn_shared(ctx, [pdf])
            if mode == "protect":
                if pdf.is_encrypted:
                    raise AdapterError(
                        "E_INTERNAL", {"reason": "alreadyEncrypted"}, retryable=False
                    )
                if ctx.secrets is None or ctx.secrets.userPassword is None:
                    raise AdapterError("E_PASSWORD_REQUIRED", {"param": "userPassword"})
                user = ctx.secrets.userPassword.get_secret_value()
                owner = (
                    ctx.secrets.ownerPassword.get_secret_value()
                    if ctx.secrets.ownerPassword is not None
                    else random_secrets.token_urlsafe(32)
                )
                encryption = pikepdf.Encryption(
                    owner=owner, user=user, R=6, aes=True, allow=permissions(params)
                )
                out = ctx.output_path("protected.pdf")
                try:
                    pages = save_pdf(ctx, pdf, out, encryption=encryption, check_password=user)
                finally:
                    del user, owner, encryption
                return [OutputFile(out, pages=pages)]

            if not pdf.is_encrypted:
                raise AdapterError("E_INTERNAL", {"reason": "notEncrypted"}, retryable=False)
            if self._needs_confirmation(ctx, ctx.inputs[0], pdf) and (
                params.get("confirmRights") is not True
            ):
                raise AdapterError("E_INTERNAL", {"reason": "confirmRights"}, retryable=False)
            out = ctx.output_path("unlocked.pdf")
            pages = save_pdf(ctx, pdf, out)
            return [OutputFile(out, pages=pages)]


if __name__ == "__main__":
    run_main(PdfProtectAdapter)
