# Contribuir a DataForge

Gracias por tu interés. DataForge es un motor de reconstrucción documental
con garantías fuertes; las contribuciones se evalúan primero por seguridad y
verificabilidad, después por funcionalidad.

## Reglas que ninguna contribución puede romper

1. El origen es inmutable: nada escribe, renombra ni borra dentro de un
   source root.
2. No existe borrado ni sobrescritura de archivos de usuario en el MVP.
3. SQLite es la única fuente de verdad; los archivos JSON/CSV son derivados.
4. Todo cambio de estado usa la máquina de estados y genera evento de
   auditoría en la misma transacción.
5. CLI y UI solo hablan con `df-facade`.
6. Nada se declara "hecho" sin pruebas ejecutadas.
7. No se adelantan tecnologías de milestones futuros (FastCDC, búsqueda,
   plugins, IA…) sin su hito.

## Flujo

1. Para cambios no triviales, abre un issue; para cambios de arquitectura,
   una ADR (`docs/adr/TEMPLATE.md`); para cambios de garantías o formatos,
   una RFC (`docs/rfcs/TEMPLATE.md`). Criterios en RFC-0001 §50.
2. Crea una rama desde `main`.
3. Antes de abrir el PR, pasa la puerta de calidad completa:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
pnpm --filter dataforge-desktop typecheck
pnpm --filter dataforge-desktop build
```

4. El PR debe describir qué pruebas se ejecutaron y su resultado.

## Developer Certificate of Origin (DCO)

Este proyecto usa el [DCO 1.1](https://developercertificate.org/) en lugar de
CLA. Firma cada commit con `git commit -s` (añade la línea
`Signed-off-by: Nombre <email>`), afirmando que tienes derecho a aportar el
código bajo las licencias del proyecto (MIT OR Apache-2.0).

## Estilo

- Rust: `rustfmt` por defecto; clippy sin warnings; errores tipados vía
  `df-error` (nada de `unwrap()` fuera de tests).
- TypeScript: `strict` completo; sin `any`.
- Commits: imperativo, ámbito claro (`df-db: enforce append-only triggers`).
- Documentación de decisiones: ADR numerada; índice en `docs/adr/README.md`.

## Qué NO aceptamos

- Funcionalidad simulada, botones sin efecto real o estados falsos.
- Reducciones de seguridad "para acelerar el desarrollo" (regla 20 del RFC).
- Dependencias de procedencia dudosa o con licencias incompatibles
  (`deny.toml` es la referencia).
