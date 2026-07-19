# ADR-0035 — Snapshots incrementales por identidad física probada (M0.8)

**Estado:** Aceptada
**Fecha:** 2026-07-20
**Relacionada con:** RFC-0001 §14.4, §45 M0.8; ADR-0019, ADR-0029

## Contexto

Rehacer el inventario de un corpus grande costaba lo mismo que la primera
vez: la máquina de estados era estrictamente lineal (ningún estado
posterior a `SCANNED` permitía re-escanear) y cada hash releía todos los
bytes. Para el mantenimiento continuo de colecciones reales — el caso de
uso central del producto — eso convierte cada revisión en horas.

## Decisiones

1. **Los estados completados son puntos de control, no tumbas.** `HASHED`,
   `ANALYZED`, `COMPLETED` y `COMPLETED_WITH_WARNINGS` permiten iniciar un
   nuevo escaneo (snapshot nuevo; el anterior queda inmutable). Los estados
   con un plan en vuelo — de `PLAN_READY` a `EXECUTED` — siguen prohibiendo
   el rescan: un plan aprobado fija su snapshot hasta ejecutarse y
   verificarse. `FAILED` y `ARCHIVED` siguen siendo terminales.

2. **El reuso exige la identidad más fuerte que existe sin leer bytes.**
   Un binding de contenido solo se transporta del snapshot anterior cuando
   el fingerprint v2 es **byte-idéntico y con todos los campos presentes**
   (tamaño, mtime, ctime, atributos, volumen y file id — ADR-0019). Un
   token v1, o cualquier campo `none` (filesystems sin identidad física,
   típico NAS), nunca reusa: esos archivos van al hash completo. ctime y
   file id hacen que fabricar un fingerprint idéntico tras editar un
   archivo sea impracticable en NTFS.

3. **Opt-in explícito, nunca por defecto.** El §14.4 recomienda modo
   completo para los perfiles probatorios. `--incremental` es una decisión
   por ejecución; sin el flag, el comportamiento es exactamente el
   anterior (y los tests lo fijan).

4. **La procedencia es parte de la evidencia.** La migración 0019 añade
   `occurrence_content.reused_from_snapshot`: cada binding transportado
   dice de qué snapshot vino, y el evento `HASH_COMPLETED` registra
   `reused_from_previous_snapshot`. Una identidad reusada jamás se
   confunde con una recién calculada.

## Alternativas consideradas

- **Reusar con igualdad size+mtime (v1)** — descartado: es exactamente la
  sustitución que ADR-0019 existe para detectar.
- **Reuso por defecto** — descartado por §14.4 y por sorpresa silenciosa:
  cambiar el significado de `hash` sin flag alteraría la evidencia
  esperada por los flujos existentes.
- **Comparación parseada de fingerprints** — descartada en favor de la
  igualdad textual del token canónico: más estricta (cualquier diferencia
  de forma invalida) y ejecutable como un JOIN dentro de SQLite.

## Consecuencias

- Un rescan de corpus sin cambios pasa de releer todos los bytes a un
  JOIN + inserción en SQLite; solo los archivos nuevos o cambiados pagan
  hash completo.
- La cola `hash_jobs` cierra los trabajos reusados como `HASHED`, así que
  reanudación, resúmenes y análisis posteriores funcionan sin cambios.
- Deuda declarada: el escaneo en sí sigue recorriendo el árbol completo
  (el reuso empieza en el hash); un walker incremental por directorios es
  una evolución posterior de esta misma ADR.

## Tests

`df-hash`: rescan sin cambios reusa los 3 bindings y la deduplicación
resuelve idéntico a través de identidad transportada; cambiar un archivo
reusa 2 y re-hashea el cambiado; el modo por defecto nunca reusa.
`df-domain`: los completados solo reabren hacia `SCANNING`, un plan en
vuelo bloquea el rescan, y `FAILED`/`ARCHIVED` siguen sin salidas.
