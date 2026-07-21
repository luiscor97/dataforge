import type { ErrorDto } from "../types";

interface ErrorAlertProps {
  error: ErrorDto;
}

/**
 * Presents a facade error with the human-readable message first and the
 * technical code de-emphasised. Both values come verbatim from the engine:
 * the shell never rewrites nor invents what the backend reported.
 */
export function ErrorAlert({ error }: ErrorAlertProps): React.JSX.Element {
  return (
    <div className="error" role="alert">
      <p className="error-title">No se pudo completar la operación</p>
      <p className="error-message">{error.message}</p>
      <p className="error-code">
        Código técnico: <code>{error.code}</code>
      </p>
    </div>
  );
}
