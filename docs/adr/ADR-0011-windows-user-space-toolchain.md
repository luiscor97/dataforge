# ADR-0011 — Toolchain Windows en espacio de usuario (GNU + WinLibs)

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 §0.1.6 (privilegios), §0.1.11

## Contexto

La máquina de desarrollo inicial no dispone de privilegios de administrador.
Las Visual Studio Build Tools (toolchain MSVC, preferido por Rust y el único
oficialmente soportado por Tauri en Windows) requieren un instalador
machine-wide con elevación UAC. RFC-0001 §0.1.6 ordena: bloquear solo esa
instalación, documentarla, proponer la alternativa segura y continuar.

## Decisión

1. Rust se instala con `rustup` (winget `Rustlang.Rustup`, espacio de
   usuario) y el host por defecto se fija a `x86_64-pc-windows-gnu`
   (`rustup set default-host x86_64-pc-windows-gnu`).
2. El compilador/enlazador C para el target GNU es MinGW-w64 GCC de
   **WinLibs** (winget `BrechtSanders.WinLibs.POSIX.MSVCRT`, paquete portable
   en espacio de usuario). La variante MSVCRT coincide con el CRT que usa el
   target `windows-gnu` de Rust. Esto permite compilar `rusqlite` con la
   feature `bundled` (SQLite desde fuentes C).
3. `rust-toolchain.toml` pina el canal `stable` con `rustfmt` y `clippy`,
   pero **no** pina el triple del host: los contribuyentes con MSVC usan
   MSVC (preferido), y la CI usa MSVC en `windows-latest`.
4. El shell Tauri (`dataforge-desktop`) **sí** compila, enlaza y abre con el
   toolchain GNU en esta máquina (verificado: ventana "DataForge Desktop"
   funcionando contra el dev server). Requisitos que lo hicieron posible:
   `crate-type = ["rlib"]` (una cdylib supera el límite de 65 536 símbolos
   exportados del formato PE con ld de binutils) y aceptar un aviso benigno
   del linker (".rsrc merge failure: multiple non-default manifests", el
   manifest lo embebe tauri-build). MSVC sigue siendo el toolchain soportado
   oficialmente por Tauri y es el que usa la CI; los builds de distribución
   deben hacerse con MSVC.

## Acción manual pendiente (bloqueo documentado)

Instalar, cuando haya elevación disponible:
`winget install Microsoft.VisualStudio.2022.BuildTools` con la carga de
trabajo *Desktop development with C++* (MSVC v143 + Windows 11 SDK). Después:
`rustup set default-host x86_64-pc-windows-msvc`.

## Consecuencias

- El desarrollo del motor y de la CLI es 100 % funcional sin elevación.
- Existen dos triples en juego; `deny.toml` y la CI cubren ambos targets.
- Los scripts (`scripts/install-dev-tools.ps1`) detectan MSVC y solo aplican
  el fallback GNU cuando falta.
