# ADR-0012 — Política de Node.js y pnpm

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 §0.1.1, §0.1.4

## Contexto

El frontend del escritorio (React + TypeScript + Vite) necesita Node.js y un
gestor de paquetes de workspace. Node.js 24 LTS ya estaba instalado en la
máquina. `corepack enable` escribe shims en el directorio de instalación de
Node (`C:\Program Files\nodejs`), que exige administrador.

## Decisión

- Se reutiliza el Node.js LTS preexistente (24.x); no se reinstala.
- pnpm 10 se instala con `npm install -g pnpm@10`, que en Windows escribe en
  el prefix de usuario (`%APPDATA%\npm`) sin elevación. Registro oficial npm.
- `package.json#packageManager` pina la versión exacta de pnpm
  (reproducibilidad para quienes usan Corepack).
- `.npmrc` fija `node-linker=hoisted`: evita el uso masivo de junctions de
  pnpm, que interacciona mal con carpetas sincronizadas por OneDrive como la
  copia de trabajo actual.

## Consecuencias

- Sin dependencia de permisos de administrador para el stack JS.
- `node_modules` hoisted ocupa más disco que el layout simbólico de pnpm;
  aceptado a cambio de robustez bajo OneDrive.
- Si el repositorio se mueve fuera de OneDrive, puede retirarse
  `node-linker=hoisted` mediante una ADR breve.
