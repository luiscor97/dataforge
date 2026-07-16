# ADR-0029 — Marcador de análisis completo y recuperación de fases

**Estado:** Aceptada
**Fecha:** 2026-07-16
**Relacionada con:** RFC-0001 §11, §12.4–§12.8, §26; ADR-0018, ADR-0028

## Contexto

`analyze`, `plan create` y `plan approve` abarcan varias transacciones. Una
caída entre ellas puede dejar el proyecto en `ANALYZING`, `PLANNING` o
`PLAN_REVIEW` con parte del trabajo ya persistido. Rechazar esos estados hace
irrecuperable un proyecto sano; repetir ciegamente puede crear otra versión de
plan, otro manifiesto o eventos duplicados.

Además, la ausencia de filas no distingue un informe legítimamente vacío de
una etapa que todavía no se ejecutó. El evento histórico
`ANALYSIS_COMPLETED` se emite al materializar duplicados y no certifica que
hayan terminado las etapas estructurales posteriores.

## Decisión

1. **Existe un marcador final por snapshot.** `analysis_completions`
   (migración `0010_structural_review.sql`) guarda `snapshot_id`, proyecto,
   perfil y resumen canónico después de completar duplicados, firmas,
   contextos, relaciones, representantes, reglas y anomalías. Es append-only y
   tiene una única fila por snapshot.

2. **El evento final se emite una sola vez.** Insertar por primera vez el
   marcador añade `STRUCTURAL_ANALYSIS_COMPLETED` en la misma transacción. Un
   reintento usa `INSERT OR IGNORE`, conserva la primera evidencia y no añade
   otro evento final.

3. **Los informes fallan cerrados.** Los informes del último snapshot completo
   solo se exponen cuando existe su marcador y el proyecto está en un estado
   estable igual o posterior a `ANALYZED`. Una caída en `ANALYZING`, incluso
   después de persistir alguna tabla derivada, no se presenta como un informe
   vacío válido.

4. **`analyze` se reanuda desde `ANALYZING`.** Desde `HASHED` realiza una sola
   transición inicial; desde `ANALYZING` no la repite. Las etapas derivadas se
   reejecutan de forma idempotente. Las evidencias automáticas nuevas usan ids
   estables e inserciones ignorando duplicados; las decisiones humanas viven
   aparte y no se borran.

5. **`plan create` se reanuda desde `PLANNING`.** Si todavía no hay plan,
   genera el siguiente normalmente. Si una caída dejó un plan `READY` del
   mismo snapshot, reconstruye en memoria las operaciones con la política del
   reintento y las compara ignorando solo los UUID aleatorios. Si coinciden,
   reutiliza id, versión, operaciones y evento `PLAN_CREATED`, y completa
   `PLANNING → PLAN_READY`. Si no coinciden, devuelve conflicto en vez de
   adivinar la política original o crear otra versión.

6. **`plan approve` se reanuda desde `PLAN_REVIEW`.** Si el plan sigue
   `READY`, construye y persiste el manifiesto. Si ya está `APPROVED`, valida
   que todas las operaciones estén aprobadas, que el manifiesto corresponda a
   ellas y que su SHA-256 coincida con el almacenado; después completa la
   transición del proyecto. No vuelve a insertar el manifiesto ni a emitir
   `PLAN_APPROVED`.

7. **Las incoherencias no se reparan por aproximación.** Un plan de otro
   snapshot, un estado de plan inesperado, operaciones distintas para la
   política reintentada o un manifiesto/hash incoherente producen error de
   conflicto. Recuperar significa continuar evidencia demostrable, no fabricar
   una historia plausible.

## Alternativas consideradas

- **Usar el último evento de una etapa como marcador** — descartado: el orden
  de etapas puede evolucionar y un evento intermedio no certifica el conjunto.
- **Tratar `ANALYZING` como analizado si hay alguna tabla poblada** —
  descartado: una tabla vacía puede ser válida y una poblada puede estar
  incompleta.
- **Crear siempre una nueva versión al reintentar `PLANNING`** — descartado:
  duplica intención, eventos y operaciones tras una simple pérdida de
  respuesta.
- **Hacer idempotente `approve_plan` insertando de nuevo el manifiesto** —
  descartado: la aprobación es una congelación única; el reintento debe
  verificar y reutilizarla.
- **Una transacción única para toda la fase de análisis** — descartada: sería
  larga, retendría bloqueos y perdería todo el progreso ante una caída.

## Consecuencias

- Los cortes entre transacciones dejan estados reanudables y no generan otra
  versión de plan ni otra aprobación por el mismo trabajo ya confirmado.
- Un informe vacío significa «análisis completo sin hallazgos», no «la fase no
  llegó a ejecutarse».
- El ledger conserva los reintentos de etapas, pero los hitos únicos
  (`STRUCTURAL_ANALYSIS_COMPLETED`, `PLAN_CREATED` reutilizado y
  `PLAN_APPROVED`) no se duplican en las ventanas cubiertas.
- El alcance no incluye coordinación distribuida ni varios procesos
  escritores compitiendo sobre el mismo proyecto; SQLite sigue siendo la
  autoridad transaccional local.
- Condición de revisión: toda nueva etapa añadida a `analyze` debe ejecutarse
  antes del marcador y demostrar que su reintento no borra decisiones humanas.
