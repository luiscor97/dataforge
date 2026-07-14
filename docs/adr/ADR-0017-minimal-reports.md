# ADR-0017 — Informes mínimos exportables (M0.1)

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §35, §36, §28, §15; reglas 5 y 6 del §0

## Contexto

El Milestone 0.1 incluye "informes mínimos" entre sus capacidades. Hasta
ahora todo el estado vivía en SQLite pero nada se **exportaba** a disco. El
RFC §35 define la estructura de proyecto (`plans/`, `reports/`) y el §36 un
encabezado común versionado. Faltaba materializar el plan, el manifiesto de
verificación y la evidencia de duplicados como archivos.

## Decisiones

1. **Los informes son exportaciones, no fuente de verdad (regla 6).** SQLite
   es la única fuente transaccional (regla 5); `df-report` renderiza una
   vista de solo lectura. Regenerar un informe **sobrescribe** el archivo
   homónimo, y eso no viola la regla de no-sobrescritura: esa regla protege
   el origen y la salida documental, nunca las exportaciones regenerables de
   `plans/` y `reports/`.

2. **Escritura atómica (temp + rename).** Cada exportación se escribe a un
   `.<nombre>.tmp` que se vuelca con `sync_all` y se renombra sobre el
   destino, de modo que un fallo a mitad nunca deja un JSON corrupto. En
   Windows el rename exige quitar antes el destino previo (un informe
   regenerable, no dato de usuario).

3. **Encabezado común versionado (§36.1) en todo JSON.** `schema`,
   `schema_version` (SemVer propio, `1.0.0`), `project_id`, `snapshot_id`,
   `created_at`, `generator_version`. Los esquemas: `dataforge.plan`,
   `dataforge.report`, `dataforge.duplicates`.

4. **Numeración por versión de plan.** `plans/plan-NNNN.json` y
   `reports/verification-NNNN.json` usan `NNNN` = versión del plan, así que
   re-exportar el mismo plan es idempotente y un re-plan produce archivos
   nuevos. Los duplicados usan el identificador corto del snapshot.

5. **El informe de verificación incluye el manifiesto de copia.** Además del
   veredicto, la cobertura y los hallazgos, lista cada artefacto con su
   destino y su SHA-256 — la prueba auditable de la migración (§28). Se
   emite en JSON (máquina) y en Markdown legible (persona). La raíz Merkle
   opcional del §29.3 queda para una fase posterior.

## Alternativas consideradas

- Escribir con `std::fs::write` directo — descartado: deja archivos a medias
  si el proceso muere; el temp+rename es barato y robusto.
- Un contador incremental de informes buscando el siguiente hueco —
  descartado: la versión del plan da un nombre determinista y estable.
- Exportar PDF — fuera de alcance de "informes mínimos"; el §36 admite PDF
  pero JSON/CSV/Markdown cubren el criterio sin dependencias pesadas.

## Consecuencias

- Cierra la capacidad "informes mínimos" de M0.1: el usuario obtiene un plan
  y un informe de migración auditables fuera de la base de datos.
- `df-report` es un crate de solo lectura sobre `df-db`; no emite eventos ni
  cambia el estado del proyecto.
- Deuda aceptada: raíz Merkle del manifiesto (§29.3), exportación PDF y
  firma Ed25519 (§29.4). Revisar en fases posteriores.
