# ADR-0015 — Decisiones del incremento de inventario (M0.1): escaneo y hashing

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §9.3, §12.1–§12.3, §13, §14, §15; ADR-0003, ADR-0007 (RFC §6)

## Contexto

El primer incremento del Milestone 0.1 implementa escaneo, fingerprints,
hashing BLAKE3/SHA-256 y duplicados exactos. Varias decisiones de detalle no
están fijadas por el RFC y deben quedar registradas.

## Decisiones

1. **Rutas relativas en almacenamiento.** `path_occurrences` y `folders`
   guardan la ruta relativa a su `source_root`; la ruta absoluta se
   reconstruye al presentar datos. Evita duplicar cientos de miles de
   prefijos y deja el inventario coherente si un origen se re-monta con otra
   letra de unidad. La entidad del RFC §9.3 expone `absolute_path`: se
   satisface como dato derivado, no como columna.

2. **Estado de carpeta según legibilidad.** La fila de una carpeta se
   escribe al desencolarla, con `OK` si `read_dir` funcionó y `ERROR` con el
   texto del fallo si no. Los errores parciales nunca abortan el escaneo
   (RFC §13.1).

3. **Cancelar un escaneo invalida su snapshot.** Un snapshot es íntegro o no
   es (regla 4): al cancelar, el snapshot queda `FAILED`, el proyecto pasa a
   `SCAN_PAUSED` y reanudar crea un snapshot nuevo desde cero. La
   reanudación con checkpoints del RFC §13.1 queda para cuando existan
   volúmenes que la justifiquen; el hashing sí reanuda de verdad.

4. **Fingerprint v1 = tamaño + mtime.** Token versionado
   (`v1:<size>:<mtime_ms>`). La identidad física de Windows (volume serial +
   file index, RFC §13.5) requiere llamadas fuera de std y se incorporará
   como `v2` sin colisionar con tokens antiguos.

5. **Hashing en modo completo y una sola pasada.** Cada archivo se lee una
   vez alimentando BLAKE3 y SHA-256 a la vez. El modo rápido en dos pasos
   del RFC §14.4 es una optimización posterior; el modo completo es el
   recomendado para el perfil jurídico y el más simple de auditar.

6. **Validación no mata el proyecto.** Si la validación (§12.1) falla, el
   proyecto permanece en `VALIDATING` y el error se devuelve al usuario.
   `FAILED` es terminal y un origen temporalmente inaccesible (unidad de
   red, USB) no debe destruir el proyecto.

7. **Duplicados como consulta, no como tabla.** Los duplicados exactos
   (mismo tamaño + mismo SHA-256) se derivan con una consulta sobre
   `occurrence_content`. La tabla `duplicate_sets` del RFC §10.1 llegará con
   las políticas de duplicado del planner, que es quien materializa
   decisiones.

## Alternativas consideradas

- Guardar `absolute_path` por fila — descartada por redundancia y riesgo de
  desincronización.
- Reanudar escaneos parciales con checkpoints — descartada en este
  incremento por complejidad frente a beneficio con los volúmenes objetivo.
- Hash BLAKE3 primero y SHA-256 solo para relaciones relevantes (modo
  rápido §14.4) — pospuesta; el modo completo cubre el criterio de 100k
  archivos y simplifica la verificación.

## Consecuencias

- El motor completa `VALIDATE → SNAPSHOT → HASH` con auditoría íntegra.
- Interrumpir un hash (kill, corte) no pierde trabajo: la cola `hash_jobs`
  es persistente e idempotente.
- Deuda aceptada: reanudación de escaneo por checkpoint, identidad física
  v2, modo rápido de hashing, tabla `duplicate_sets`. Revisar al preparar
  el corpus de 100.000 archivos del criterio de aceptación de M0.1.
