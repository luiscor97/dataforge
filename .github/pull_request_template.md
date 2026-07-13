## Qué cambia

<!-- Descripción breve y motivación. Enlaza issue/ADR/RFC si aplica. -->

## Tipo de cambio

- [ ] Corrección
- [ ] Funcionalidad del milestone actual
- [ ] Documentación / ADR / RFC
- [ ] Infraestructura (CI, scripts, tooling)

## Puerta de calidad (obligatoria)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test` (pega el resumen abajo)
- [ ] `pnpm --filter dataforge-desktop typecheck && pnpm --filter dataforge-desktop build` (si toca el escritorio)

```text
<resumen de tests>
```

## Invariantes

- [ ] No introduce borrado ni sobrescritura de archivos de usuario.
- [ ] No escribe dentro de source roots.
- [ ] Cambios de estado pasan por la máquina de estados + evento de auditoría.
- [ ] CLI/UI siguen usando solo `df-facade`.
- [ ] No adelanta tecnología de milestones futuros.

## DCO

- [ ] Commits firmados con `git commit -s` (Signed-off-by).
