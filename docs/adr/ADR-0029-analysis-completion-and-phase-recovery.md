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
   tiene una única fila por `(snapshot, versión de análisis)`; los consumidores
   exigen la versión actual.

2. **El evento final se emite una sola vez.** Insertar por primera vez el
   marcador añade `STRUCTURAL_ANALYSIS_COMPLETED` en la misma transacción. Un
   reintento usa `ON CONFLICT DO NOTHING`, compara proyecto, versión, perfil,
   digest y resumen con la fila existente, y no añade otro evento final.

3. **Los informes fallan cerrados.** Los informes del último snapshot completo
   solo se exponen cuando existe su marcador y el proyecto está en un estado
   estable igual o posterior a `ANALYZED`. Una caída en `ANALYZING`, incluso
   después de persistir alguna tabla derivada, no se presenta como un informe
   vacío válido.

4. **`analyze` se reanuda desde `ANALYZING`.** Desde `HASHED` realiza una sola
   transición inicial; desde `ANALYZING` no la repite. Si todavía no existe el
   marcador final, las etapas derivadas se reejecutan de forma idempotente. Si
   el marcador ya fue confirmado pero la caída ocurrió antes de
   `ANALYZING → ANALYZED`, no se reescribe ninguna tabla sellada: se validan
   proyecto, snapshot, versión, perfil y digest, se contrasta el resumen
   canónico con consultas sobre la evidencia inmutable, se reconstruye de él
   el mismo resultado público y se completa únicamente la transición que
   falta. Una incoherencia falla cerrada. Para marcadores v1 anteriores al
   campo `candidate_cap_reached`, la recuperación acepta el booleano histórico
   `pairs_skipped` como alias de compatibilidad; todo marcador nuevo persiste y
   sella únicamente el nombre vigente.

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

8. **El marcador sella la evidencia automática.** La migración
   `0011_derived_evidence_seal.sql` impide `INSERT`, `UPDATE` y `DELETE` para el
   snapshot completado en `duplicate_sets`, `folder_signatures`,
   `tree_clone_sets`, `folder_contexts`, `tree_relations` y
   `duplicate_representatives`. `0010` ya aplica el mismo principio a reglas,
   anomalías e ítems de revisión. Los triggers comprueban tanto `OLD` como
   `NEW` al actualizar, por lo que tampoco se puede mover evidencia hacia o
   desde un snapshot sellado. `review_decisions` queda deliberadamente fuera:
   es el flujo humano append-only posterior al análisis.

9. **Una versión nueva exige un snapshot nuevo.** Las tablas estructurales
   selladas no llevan versión por fila y su significado histórico no se
   reinterpreta. Si aumenta `ANALYSIS_VERSION`, un snapshot que ya tenga un
   marcador de otra versión no se recalcula ni se migra en sitio: el operador
   debe crear un proyecto nuevo sobre las mismas fuentes mediante el flujo
   soportado de creación y ejecutar allí `scan → hash → analyze`, obteniendo un
   snapshot nuevo con el contrato vigente. El estado `ANALYZED` del proyecto
   sellado no admite volver directamente a `scan`.

10. **El cierre de ejecución es atómico y recupera el formato anterior.** Un
    proyecto que quedó en `EXECUTING` puede reanudar sus operaciones `RUNNING`
    bajo el modelo local de un solo escritor. El hito
    `EXECUTION_COMPLETED`/`EXECUTION_PAUSED`, el estado final del proyecto y su
    evento `STATE_CHANGED` se confirman en una sola transacción. Si una versión
    anterior cayó después de grabar el hito pero antes de cambiar el estado,
    un reintento sin trabajo nuevo reutiliza solo el último hito del mismo plan;
    cualquier reintento que sí ejecute una operación registra un hito nuevo.

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
- El resumen final no puede divergir de las tablas derivadas por una repetición
  tardía ni por escritura SQLite directa; un snapshot nuevo sigue abierto hasta
  recibir su propio marcador.
- La actualización del algoritmo de análisis conserva los snapshots sellados:
  incrementar `ANALYSIS_VERSION` implica un proyecto/snapshot nuevo mediante
  el flujo soportado, no reescribir evidencia histórica.
- El ledger conserva los reintentos de etapas, pero los hitos únicos
  (`STRUCTURAL_ANALYSIS_COMPLETED`, `PLAN_CREATED` reutilizado y
  `PLAN_APPROVED`) no se duplican en las ventanas cubiertas.
- El alcance no incluye coordinación distribuida ni varios procesos
  escritores compitiendo sobre el mismo proyecto; SQLite sigue siendo la
  autoridad transaccional local.
- Condición de revisión: toda nueva etapa añadida a `analyze` debe ejecutarse
  antes del marcador, añadir su tabla automática al sellado y demostrar que su
  reintento no borra decisiones humanas.
