# ADR-0023 — Firmas Merkle de carpeta y detección de clones exactos de árbol

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §19, §18, ADR-0007

## Contexto

RFC-0001 §19 plantea el problema de los "árboles injertados": carpetas
completas que reaparecen dentro de sí mismas, dentro de otra materia,
copiadas desde backups, renombradas o parcialmente mezcladas. El §19.2 fija
una firma Merkle por carpeta como mecanismo de detección, y el §19.3 nombra
cinco relaciones posibles entre carpetas (`EXACT_TREE_CLONE`,
`PARTIAL_TREE_CLONE`, `TREE_EMBEDDED`, `REPEATED_COMPONENT_ONLY`,
`UNIQUE_CONTENT_IN_CLONE`), pero no fija el algoritmo de codificación de
entradas ni qué alcance implementar primero. El primer incremento de
Milestone 0.2 debe cerrar esas decisiones y entregar la variante más simple
y más segura del §19.3.

## Decisión(es)

1. **Codificación de entradas y algoritmo de la firma.** La firma de una
   carpeta se calcula de abajo hacia arriba:

   ```text
   folder_signature = BLAKE3( sorted( entry(child) for child in folder ) )
   entry(file)   = "F\0" + normalized_name + "\0" + sha256
   entry(folder) = "D\0" + normalized_name + "\0" + child_folder_signature
   ```

   El separador es el byte NUL, ilegal en nombres de archivo en todos los
   sistemas de archivos soportados, lo que hace la codificación
   a prueba de inyección: no existe combinación de nombre y hash que pueda
   fabricar una entrada falsa. Las entradas se ordenan antes de hashear, de
   modo que la firma es independiente del orden de lectura del directorio.

2. **BLAKE3 para la firma, SHA-256 como identidad de contenido de entrada.**
   La firma de carpeta usa BLAKE3, consistente con ADR-0007 (RFC-0001 §6):
   BLAKE3 es el hash operativo de DataForge para caché, árboles y chunks;
   SHA-256 sigue siendo la identidad canónica de auditoría por archivo. Las
   entradas de tipo archivo llevan el SHA-256 ya calculado por `df-hash`
   (§14) como identidad de contenido; la firma de carpeta no vuelve a leer
   bytes de archivo, solo combina hashes ya existentes.

3. **Regla de completitud (seguridad, §19.4).** La firma de una carpeta es
   válida (`is_complete = true`, `signature` no nulo) solo si todos sus
   archivos descendientes tienen hash de contenido y ningún archivo o
   subcarpeta del subárbol quedó en error o es un reparse point no seguido.
   Si falta una sola condición, la carpeta y todos sus ancestros quedan
   `is_complete = false` con `signature = NULL`. Solo carpetas completas
   participan en la detección de clones: una rama parcialmente escaneada o
   parcialmente hasheada nunca se declara idéntica a otra, aunque coincida
   en lo que sí se ha observado hasta el momento.

4. **Alcance de esta rebanada: solo `EXACT_TREE_CLONE`.** Dos o más carpetas
   completas y no vacías que comparten la misma firma forman un conjunto
   `EXACT_TREE_CLONE`. Las relaciones `PARTIAL_TREE_CLONE`, `TREE_EMBEDDED`,
   `REPEATED_COMPONENT_ONLY` y `UNIQUE_CONTENT_IN_CLONE` del §19.3 quedan
   nombradas en el vocabulario (`TreeRelationship`) para que sea estable,
   pero su cómputo se aplaza a una rebanada posterior.

5. **Solo informe, sin proponer ni ejecutar nada.** La detección de clones
   de árbol es evidencia: lista los conjuntos y los bytes redundantes que
   implicarían, pero no genera operaciones de plan ni marca ninguna copia
   para eliminación. El §19.4 prohíbe retirar una rama completa antes de
   identificar su contenido único, y ese análisis depende de perfiles y del
   grafo contextual (§18), que no existen todavía. La consolidación se
   pospone a una rebanada con esos prerrequisitos.

6. **Dónde se ejecuta.** El cómputo corre dentro del paso `analyze` ya
   existente, inmediatamente después de materializar los duplicados exactos
   (§15), como parte de la transición `HASHED → ANALYZING → ANALYZED`. Se
   persiste en dos tablas nuevas de la migración `0004_structure.sql`:
   `folder_signatures` (una fila por carpeta del snapshot, con
   `signature`/`is_complete`/tamaño de subárbol) y `tree_clone_sets`
   (conjuntos materializados de dos o más carpetas con la misma firma). El
   recómputo es idempotente: sustituye las filas del snapshot, de forma que
   volver a analizar tras hashear más archivos simplemente actualiza la
   evidencia. Se emite el evento de auditoría `STRUCTURE_ANALYZED` con el
   recuento de carpetas firmadas, carpetas completas y conjuntos de clones.
   Un nuevo informe de CLI, `dataforge report tree-clones`, lista los
   conjuntos detectados.

## Alternativas consideradas

- **Hashear nombre y tipo por separado, sin separador NUL** — descartada:
  abre la puerta a colisiones de codificación entre, por ejemplo, un archivo
  `"ab"` + hash `"c"` y un archivo `"a"` + hash `"bc"`; el NUL como separador
  elimina la ambigüedad porque es un byte que ningún nombre de archivo real
  puede contener.
- **SHA-256 también para la firma de carpeta** — descartada: RFC-0001 §6
  (ADR-0007) reserva SHA-256 para la identidad canónica por archivo y
  BLAKE3 para árboles y estructuras derivadas; usar SHA-256 aquí duplicaría
  el rol de BLAKE3 sin aportar nada y se apartaría de la convención ya
  fijada.
- **Marcar una carpeta como completa si "la mayoría" de sus archivos están
  hasheados** — descartada: viola directamente la regla de seguridad del
  §19.4; un umbral parcial podría declarar clon exacto algo que en realidad
  difiere en el contenido todavía no observado.
- **Implementar las cinco relaciones del §19.3 en esta misma rebanada** —
  descartada por alcance: `PARTIAL_TREE_CLONE` y `TREE_EMBEDDED` requieren
  comparar subconjuntos de entradas entre carpetas, no solo la firma
  completa, y `UNIQUE_CONTENT_IN_CLONE` requiere el resultado de esa
  comparación; `EXACT_TREE_CLONE` es la variante mínima que ya aporta valor
  como evidencia y no compromete la seguridad del §19.4.
- **Proponer consolidación (representar una copia, marcar las demás) ya en
  esta rebanada** — descartada: sin contextos ni perfiles (§18), DataForge
  no puede distinguir todavía qué copia es la "activa" ni qué rama contiene
  contenido único fuera del clon; proponer aquí sería adelantarse a la regla
  de seguridad del §19.4.

## Consecuencias

- Detección de clones de árbol demostrable de extremo a extremo, con la
  misma garantía de "no borrar antes de saber" que ya rige duplicados
  exactos: es evidencia auditable, no una acción.
- La firma de carpeta reutiliza el SHA-256 ya calculado por el hashing de
  §14 en lugar de releer archivos, así que el coste adicional del análisis
  estructural es proporcional al número de carpetas y ocurrencias del
  snapshot, no a sus bytes.
- Deuda aceptada, a registrar en el backlog de Milestone 0.2: las
  relaciones `PARTIAL_TREE_CLONE`, `TREE_EMBEDDED`, `REPEATED_COMPONENT_ONLY`
  y `UNIQUE_CONTENT_IN_CLONE`; la consolidación de duplicados guiada por
  firmas de carpeta; y el uso de firmas de carpeta en la planificación
  (§26). Ninguna de estas piezas existe todavía ni se insinúa en el plan
  generado por `df-planner`.
- Condición de revisión: cuando el grafo contextual (§18) y los perfiles
  existan, esta ADR debe revisarse para decidir si la consolidación de
  clones se apoya en las mismas tablas (`folder_signatures`,
  `tree_clone_sets`) o si requiere un esquema adicional.
