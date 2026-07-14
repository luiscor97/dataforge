# Skill: rust-quality-gate

**Nombre:** rust-quality-gate
**Objetivo:** puerta de calidad obligatoria antes de dar por terminada
cualquier tarea de código (RFC-0001 §49: nada está "hecho" sin pruebas).

## Cuándo usarla

- Antes de cada commit que toque código Rust o TypeScript.
- Al cerrar cualquier tarea o milestone.

## Entradas

- Árbol de trabajo del repositorio.

## Salidas

- Veredicto pasa/no-pasa con la salida de cada comando.

## Herramientas permitidas

- cargo (fmt, clippy, test, build), pnpm (typecheck, build).

## Límites

- Prohibido desactivar lints con `#[allow]` para "pasar la puerta" sin
  justificarlo en el propio código.
- Prohibido marcar tests como `#[ignore]` para ocultar fallos.
- Si un comando falla, la tarea NO está terminada; no se reporta éxito
  parcial como éxito.

## Comandos

```powershell
# Si el repo está en OneDrive, usa un target dir local:
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\dataforge-target"

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cargo build -p dataforge-cli
pnpm --filter dataforge-desktop typecheck
pnpm --filter dataforge-desktop build
```

## Criterios de éxito

- Los seis comandos devuelven exit 0.
- El resumen de `cargo test` se incluye en el mensaje de cierre/PR.

## Fallos esperados

- `clippy -D warnings` falla por lints nuevos tras subir de toolchain:
  corregir el código, no bajar el nivel.
- fmt falla por archivos generados: no formatear `dist/`, `target/`.
