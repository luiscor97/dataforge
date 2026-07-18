# ADR-0030 — Similitud de contenido streaming y linaje candidato (M0.3)

**Estado:** Aceptada  
**Fecha:** 2026-07-18  
**Relacionada con:** RFC-0001 §9.5, §16 y §45 M0.3; ADR-0003, ADR-0019, ADR-0029

## Contexto

SHA-256 demuestra identidad, pero no permite relacionar dos contenidos que
comparten gran parte de sus bytes. M0.3 debe detectar versiones, truncados y
recomposiciones sin convertir una aproximación en permiso para omitir datos.
También debe funcionar con archivos grandes sin cargar cada archivo completo ni
generar un producto cartesiano de todos los contenidos.

## Decisión

1. **FastCDC canónico y versionado.** Se usa `fastcdc` v3.2.1, implementación
   `v2020::StreamCDC`, normalización nivel 1 y perfil inicial 16/64/256 KiB.
   La versión exacta del algoritmo y los parámetros forman parte de cada run y
   de toda membresía de chunk. Cambiarlos crea evidencia nueva; nunca
   reinterpreta la anterior.

2. **Memoria O(chunk máximo), no O(archivo/corpus).** El chunker recibe un
   `Read`; su buffer y el `ChunkData` entregado mantienen como máximo dos
   ventanas de 256 KiB, más la firma fija. Cada chunk se hashea con BLAKE3 y se
   inserta dentro de la transacción del contenido. Al terminar se contrastan
   fingerprint, tamaño y SHA-256 total con el `ContentObject` aprobado; una
   fuente cambiada revierte íntegra esa transacción y deja el run reanudable.

3. **Chunk y aparición son entidades distintas.** `chunks` identifica
   `(algorithm_version, BLAKE3, longitud)`; `chunk_memberships` conserva
   contenido, ordinal y offset. Las membresías de un contenido se publican
   atómicamente y después son append-only; el marcador MinHash permite
   reutilizarlas en un replay. Un hash de chunk no sustituye al SHA-256
   canónico del archivo.

4. **MinHash/LSH solo genera candidatos.** Una firma MinHash determinista se
   deriva del conjunto de chunks. LSH agrupa bandas y un índice invertido de
   chunks poco frecuentes aporta un fallback conservador. Tamaño de bucket y
   número total de pares tienen límites explícitos; alcanzar el techo queda
   persistido, no se presenta como análisis exhaustivo.

5. **La decisión usa evidencia exacta.** Cada candidato se reevalúa mediante el
   multiconjunto real de chunks. `similarity = shared_bytes / union_bytes`, con
   multiplicidad y longitud. MinHash nunca determina el porcentaje publicado.

6. **Linaje es candidato, no hecho histórico.** Las relaciones tipadas
   (`LIKELY_VERSION`, `TRUNCATED_VARIANT`, `RECOMPOSED_CONTENT`,
   `SIMILAR_CONTENT`) incluyen dirección temporal cuando la evidencia de fechas
   la permite, confianza, chunks/bytes compartidos y JSON canónico. No crean una
   operación de plan ni autorizan consolidación; siempre preservan ambos
   contenidos y pueden alimentar revisión humana.

7. **Runs configurables, reanudables y sellados.** Un digest cubre parámetros y
   versión. Chunking completado se reutiliza; candidatos/relaciones de un run
   interrumpido se reconstruyen determinísticamente. Completar el run, sellar
   su evidencia y emitir el evento final ocurre en una transacción.

8. **Contratos compartidos.** Facade, CLI y desktop exponen el mismo resumen y
   las mismas relaciones. Los umbrales se validan en el motor y se reflejan en
   la salida; la UI no recalcula similitud ni linaje.

## Alternativas descartadas

- Comparar cada par de contenidos: coste cuadrático no acotado.
- Usar solo MinHash como similitud: confunde una estimación con evidencia.
- Leer archivos completos o usar mmap obligatorio: memoria proporcional al
  archivo y peor comportamiento en volúmenes/remotos.
- Tratar una relación como identidad o versión confirmada: viola `hash manda` y
  `human-in-command`.

## Consecuencias

- La persistencia crece con los chunks de contenidos elegibles, no con el
  producto cartesiano de archivos.
- Los límites introducen falsos negativos y se informan explícitamente.
- Modificar parámetros puede requerir otro run y más almacenamiento; cada run
  sigue siendo reproducible y auditable.
- La semántica documental, multimedia o de contenedores se añadirá en M0.4/M0.5;
  M0.3 relaciona bytes y fechas, no interpreta contenido.
