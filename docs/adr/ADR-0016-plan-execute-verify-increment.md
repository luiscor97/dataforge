# ADR-0016 — Decisiones del incremento de planificación, ejecución y verificación (M0.1)

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §9.9–§9.10, §15.4, §26, §27, §28; ADR-0015

## Contexto

El segundo incremento del Milestone 0.1 cierra el pipeline
`ANALYZE → PLAN → APPROVE → EXECUTE → VERIFY`. El RFC fija el protocolo por
archivo (§27.1), la cobertura (§26.2) y las invariantes de verificación
(§28.2); varios detalles de aplicación quedan a criterio de implementación
y se registran aquí.

## Decisiones

1. **Política de duplicados `REPORT_ONLY` (§15.4).** Sin contextos ni
   perfiles (M0.2), el plan replica la estructura del origen bajo
   `output_root/<nombre-raíz>/…` y copia todo lo hasheado, duplicados
   incluidos. Los `duplicate_sets` se materializan como evidencia en la
   fase de análisis.

2. **Cobertura con cuatro tipos de operación.** `COPY_ACTIVE` (contenido
   verificado), `CREATE_DIRECTORY` (estructura, preserva carpetas vacías),
   `NO_ACTION` (reparse points, explícito y justificado) y `BLOCKED`
   (ilegibles o sin identidad de contenido). Los tipos contextuales del
   §26.1 son representables desde ya pero no se emiten hasta M0.2.

3. **`COPY_WITH_SUFFIX` en dos momentos.** En planificación, para
   colisiones conocidas (nombres que solo difieren en mayúsculas); en
   ejecución, para destinos preexistentes con hash distinto (§27.3). El
   sufijo es determinista: `~df-<8 hex del SHA-256>` antes de la extensión.
   Planner y executor comparten el mismo constructor: si el componente
   excedería 255 unidades UTF-16, recortan determinísticamente el stem sin
   partir un carácter y conservan la extensión completa siempre que quepa.
   La ruta real queda registrada en `operation_results.final_relative_path`;
   el plan aprobado no se modifica.

4. **Inmutabilidad del plan reforzada en tres capas.** Trigger SQL que
   bloquea UPDATE de los campos congelados cuando el plan está `APPROVED`;
   prohibición de DELETE en `plans`/`plan_operations`; y re-serialización
   canónica en la verificación, que compara contra el SHA-256 registrado al
   aprobar (`PLAN_TAMPERED` si difiere).

5. **El hash de la copia se calcula en streaming durante la escritura.**
   El §27.1 pide "hash partial": se hashea el flujo que se escribe al
   parcial y se compara con la identidad registrada en el snapshot. La
   relectura independiente desde disco la hace la fase VERIFY, que re-hashea
   cada destino. Evita duplicar la E/S en ejecución sin perder la
   comprobación de extremo a extremo.

6. **Los parciales solo se eliminan con propiedad física demostrada.** La
   regla "el MVP no borra archivos" protege datos del usuario. Cada intento
   reserva un token aleatorio y usa
   `.dataforge-partial-<operation-id>-<lease-token>`; tras ganar `create_new`
   captura la identidad física desde ese mismo handle y solo entonces la
   persiste como claim. Reanudar exige simultáneamente `RUNNING`, token e
   identidad coincidente, además de archivo regular y ausencia de reparse
   point. El borrado y el finalize validan y actúan sobre el mismo handle.
   Estado, nombre o token sin identidad nunca autorizan borrar. Si el proceso
   cae en la estrecha ventana `create_new`→claim, queda un huérfano conservador:
   no se adjudica ni se borra automáticamente y VERIFY lo reporta como
   `PARTIAL_LEFTOVER` para inspección/limpieza manual. El componente no repite
   el nombre documental, que puede ocupar las 255 unidades UTF-16 de NTFS.

   Si retirar un claim válido falla por I/O transitorio, el resultado se
   journaliza pero la operación permanece `RUNNING` y conserva token+identidad
   hasta que otro reintento confirme el borrado. Solo una ausencia confirmada
   o un conflicto de identidad/reparse (entrada extranjera) permiten limpiar
   el claim sin borrar esa entrada.

7. **Fallos clasificados como en §27.4/§27.5.** `NO_SPACE` e `IO_ERROR`
   son `FAILED_RETRYABLE` (reintento en la siguiente ejecución);
   `SOURCE_CHANGED`, `SOURCE_MISSING`, `PERMISSION_DENIED`, `HASH_MISMATCH`,
   `DESTINATION_CHANGED` e `INVALID_PATH` son `FAILED_FINAL`. Una operación
   `RUNNING` huérfana (proceso matado) se reintenta.

8. **`EXECUTED` exige estados terminales, no éxito total.** Si quedan
   operaciones pendientes o retryables el proyecto va a `EXECUTION_PAUSED`
   (reanudable); con todo terminal pasa a `EXECUTED` aunque haya
   `FAILED_FINAL`, y es la verificación quien emite el veredicto
   (`INCOMPLETE_OPERATION` es problema → `FAILED`).

9. **Severidad de hallazgos de verificación.** Problemas (suspenden):
   `PLAN_TAMPERED`, `INCOMPLETE_OPERATION`, `MISSING_DESTINATION`,
   `HASH_MISMATCH`, `PARTIAL_LEFTOVER`. Avisos (degradan a
   `COMPLETED_WITH_WARNINGS`): `ORIGIN_CHANGED`, `UNTRACKED_FILE` — indican
   cambios externos, no fallos de DataForge.

10. **Comprobación previa de espacio en disco: aplazada.** std no expone el
    espacio libre y no se añade una dependencia solo para el pre-check; el
    fallo real se captura como `NO_SPACE` retryable por operación. Se
    revisará con el corpus de 100k (exit code 6 sigue reservado).

## Alternativas consideradas

- Materializar la pertenencia de los duplicate_sets en tabla propia —
  descartada: derivable de `occurrence_content`.
- Re-leer el parcial desde disco antes del rename — descartada en este
  incremento (duplica E/S); VERIFY re-lee todo de forma independiente.
- Bloquear `EXECUTED` si existe cualquier fallo — descartada: dejaría
  proyectos irrecuperables ante un único `SOURCE_CHANGED`; la verificación
  ya lo convierte en veredicto `FAILED` con evidencia.

## Consecuencias

- La promesa mínima del RFC §1 es demostrable de extremo a extremo con
  evidencia criptográfica (26 eventos encadenados en la prueba de humo).
- Deuda aceptada: pre-check de espacio, tipos de operación contextuales,
  raíz Merkle del manifiesto (§29.3), exportación de informes (§35
  `plans/`, `reports/`), UI de revisión del plan. Registrar en el backlog
  de M0.2 o del cierre de M0.1 (corpus 100k).
