# ADR-0023 — Firmas Merkle de carpeta y detección de clones exactos de árbol

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §19, §18; ADR-0027

**Revisada:** 2026-07-16 para reflejar que ADR-0027 amplió el análisis con
relaciones acotadas; 2026-07-17 para documentar la identidad raw usada en las
entradas Merkle y la materialización de `REPEATED_COMPONENT_ONLY`.

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
   merkle_name   = "raw:" + hex(UTF16LE(raw_basename))
                 | "display:" + normalized_name  # fallback legacy no-lossy
   entry(file)   = "F\0" + merkle_name + "\0" + sha256
   entry(folder) = "D\0" + merkle_name + "\0" + child_folder_signature
   ```

   Cuando existe `raw_relative_path`, solo su componente final participa y se
   codifican exactamente sus unidades UTF-16LE; así dos nombres con distinto
   raw no forman un clon falso aunque compartan representación visual. El
   namespace `display:` se reserva para snapshots heredados sin raw y solo se
   acepta si el nombre no es lossy. Los namespaces no pueden colisionar entre
   sí. El separador es el byte NUL, ilegal en nombres de archivo en todos los
   sistemas de archivos soportados, lo que hace la codificación a prueba de
   inyección. Las entradas se ordenan antes de hashear, de modo que la firma es
   independiente del orden de lectura del directorio.

2. **BLAKE3 para la firma, SHA-256 como identidad de contenido de entrada.**
   La firma de carpeta usa BLAKE3, consistente con RFC-0001 §6:
   BLAKE3 es el hash operativo de DataForge para caché, árboles y chunks;
   SHA-256 sigue siendo la identidad canónica de auditoría por archivo. Las
   entradas de tipo archivo llevan el SHA-256 ya calculado por `df-hash`
   (§14) como identidad de contenido; la firma de carpeta no vuelve a leer
   bytes de archivo, solo combina hashes ya existentes.

3. **Regla de completitud (seguridad, §19.4).** La firma de una carpeta es
   válida (`is_complete = true`, `signature` no nulo) solo si todos sus
   archivos descendientes tienen hash de contenido, todo nombre lossy conserva
   identidad raw y ningún archivo o subcarpeta del subárbol quedó en error o es
   un reparse point no seguido.
   Si falta una sola condición, la carpeta y todos sus ancestros quedan
   `is_complete = false` con `signature = NULL`. Solo carpetas completas
   participan en la detección de clones: una rama parcialmente escaneada o
   parcialmente hasheada nunca se declara idéntica a otra, aunque coincida
   en lo que sí se ha observado hasta el momento.

4. **La firma resuelve `EXACT_TREE_CLONE`.** Dos o más carpetas completas y
   no vacías que comparten la misma firma forman un conjunto
   `EXACT_TREE_CLONE`. Las relaciones parciales, embebidas y de componente
   repetido no se deducen de una firma igual: ADR-0027 las calcula después
   mediante conjuntos de identidades exactas de contenido y, para los
   auto-injertos, la multiplicidad de apariciones. `REPEATED_COMPONENT_ONLY`
   se persiste solo con esa evidencia adicional y acotada; no se presenta como
   un clon accionable.

5. **Solo informe, sin proponer ni ejecutar nada.** La detección de clones
   de árbol es evidencia: lista los conjuntos y los bytes redundantes que
   implicarían, pero no genera operaciones de plan ni marca ninguna copia
   para eliminación. El §19.4 prohíbe retirar una rama completa antes de
   identificar su contenido exclusivo. La consolidación automática de ramas
   sigue fuera de alcance aun cuando ADR-0027 aporte sus recuentos: compartir
   bytes no demuestra que el contexto de una carpeta sea prescindible.

6. **Dónde se ejecuta.** El cómputo corre dentro del paso `analyze` ya
   existente, inmediatamente después de materializar los duplicados exactos
   (§15), como parte de la transición `HASHED → ANALYZING → ANALYZED`. Se
   persiste en dos tablas de la migración `0006_structure.sql`:
   `folder_signatures` (una fila por carpeta del snapshot, con
   `signature`/`is_complete`/tamaño de subárbol) y `tree_clone_sets`
   (conjuntos materializados de dos o más carpetas con la misma firma). El
   recómputo es idempotente: sustituye las filas del snapshot, de forma que
   volver a analizar tras hashear más archivos simplemente actualiza la
   evidencia. Se emite el evento de auditoría `STRUCTURE_ANALYZED` con el
   recuento de carpetas firmadas, carpetas completas y conjuntos de clones.
   El informe `dataforge report tree-clones` lista los
   conjuntos detectados.

## Alternativas consideradas

- **Hashear nombre y tipo por separado, sin separador NUL** — descartada:
  abre la puerta a colisiones de codificación entre, por ejemplo, un archivo
  `"ab"` + hash `"c"` y un archivo `"a"` + hash `"bc"`; el NUL como separador
  elimina la ambigüedad porque es un byte que ningún nombre de archivo real
  puede contener.
- **SHA-256 también para la firma de carpeta** — descartada: RFC-0001 §6
  reserva SHA-256 para la identidad canónica por archivo y
  BLAKE3 para árboles y estructuras derivadas; usar SHA-256 aquí duplicaría
  el rol de BLAKE3 sin aportar nada y se apartaría de la convención ya
  fijada.
- **Marcar una carpeta como completa si "la mayoría" de sus archivos están
  hasheados** — descartada: viola directamente la regla de seguridad del
  §19.4; un umbral parcial podría declarar clon exacto algo que en realidad
  difiere en el contenido todavía no observado.
- **Derivar todas las relaciones de la firma completa** — descartada:
  `PARTIAL_TREE_CLONE` y `TREE_EMBEDDED` exigen comparar conjuntos de
  contenidos, no firmas iguales. ADR-0027 incorpora esa comparación como una
  decisión separada, acotada y determinista.
- **Proponer consolidación (representar una copia, marcar las demás) ya en
  esta rebanada** — descartada: una firma igual prueba bytes y estructura,
  pero no demuestra que el contexto de una rama sea prescindible. Los
  perfiles incorporados después protegen límites explícitos; no convierten
  los clones de árbol en una autorización automática (§19.4).

## Consecuencias

- Detección de clones de árbol demostrable de extremo a extremo, con la
  misma garantía de "no borrar antes de saber" que ya rige duplicados
  exactos: es evidencia auditable, no una acción.
- La firma de carpeta reutiliza el SHA-256 ya calculado por el hashing de
  §14 en lugar de releer archivos, así que el coste adicional del análisis
  estructural es proporcional al número de carpetas y ocurrencias del
  snapshot, no a sus bytes.
- ADR-0027 entrega `PARTIAL_TREE_CLONE`, `TREE_EMBEDDED` y
  `REPEATED_COMPONENT_ONLY`; no cambia el hecho de que los conjuntos exactos y
  las relaciones son evidencia, no operaciones de consolidación de una rama.
- Deuda aceptada: no se listan las rutas exclusivas de cada relación parcial y
  las firmas de carpeta no autorizan omitir árboles en el plan.
- Condición de revisión: cualquier consolidación futura de árboles deberá
  demostrar cobertura del contenido exclusivo y respeto de fronteras
  protegidas; no basta con reutilizar `tree_clone_sets`.
