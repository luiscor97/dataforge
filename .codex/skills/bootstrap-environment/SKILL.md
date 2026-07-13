# Skill: bootstrap-environment

**Nombre:** bootstrap-environment
**Objetivo:** dejar una máquina Windows lista para desarrollar DataForge,
instalando solo lo que falte y verificando que funciona.

## Cuándo usarla

- Primera sesión en una máquina nueva.
- Tras un fallo de compilación por herramienta ausente ("linker not found",
  "pnpm not recognized"…).
- Antes de reportar el entorno en `docs/environment-report.md`.

## Entradas

- Ninguna obligatoria. Flags: `-SkipPlugins` en bootstrap-windows.ps1.

## Salidas

- Herramientas instaladas en espacio de usuario y verificadas.
- Código de salida 0 (todo verificado) o ≠0 con la causa impresa.

## Herramientas permitidas

- `scripts/bootstrap-windows.ps1` y los scripts que orquesta.
- winget, rustup, npm/pnpm (registros oficiales únicamente).

## Límites

- Nunca instalar con elevación UAC: si algo la exige, documentarlo en
  `docs/environment-report.md` y continuar (RFC-0001 §0.1.6).
- Nunca `curl <url> | sh` / `irm <url> | iex` (RFC-0001 §0.1.5).
- Nunca desinstalar toolchains existentes ni tocar configuración global
  ajena al proyecto.

## Comandos

```powershell
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1
# o por pasos:
powershell -ExecutionPolicy Bypass -File scripts/check-environment.ps1
powershell -ExecutionPolicy Bypass -File scripts/install-dev-tools.ps1
powershell -ExecutionPolicy Bypass -File scripts/verify-toolchain.ps1
```

## Criterios de éxito

- `verify-toolchain.ps1` termina con exit 0 (compila y ejecuta Rust real).
- `cargo build` y `pnpm install` funcionan en el repositorio.

## Fallos esperados

- Sin MSVC: se aplica el fallback GNU (ADR-0011); MSVC queda como acción
  manual pendiente.
- PATH sin refrescar en shells ya abiertos: abrir shell nuevo o re-ejecutar
  el script (los scripts ajustan el PATH de su propia sesión).
- winget sin fuente msstore/winget: ejecutar `winget source update`.
