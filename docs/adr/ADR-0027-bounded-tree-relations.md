# ADR-0027 — Relaciones estructurales acotadas entre árboles

**Estado:** Aceptada
**Fecha:** 2026-07-16
**Relacionada con:** RFC-0001 §19.3, §19.4; ADR-0023

## Contexto

ADR-0023 materializa clones exactos mediante firmas Merkle completas. Eso no
cubre dos casos frecuentes en colecciones heredadas: ramas que comparten una
parte sustancial pero conservan contenido exclusivo en ambos lados, y ramas
cuyo conjunto de contenidos está incluido en otra más amplia. Confundir
cualquiera de ellas con un duplicado descartable vulneraría RFC-0001 §19.4.

Comparar todas las carpetas entre sí tampoco es aceptable. El coste sería
cuadrático, los componentes ubicuos —logos, plantillas o licencias— producirían
ruido, y una selección dependiente del orden de un `HashMap` no sería
reproducible.

## Decisión

1. **La unidad de comparación es el contenido exacto.** Para cada carpeta
   completa se forma el conjunto de `content_id` distintos de todo su
   subárbol. Solo intervienen apariciones inventariadas correctamente y con
   identidad de contenido. No se comparan nombres, texto extraído ni
   características visuales.

2. **Solo se comparan ramas completas y no triviales.** Una carpeta necesita
   al menos dos contenidos distintos. Las carpetas incompletas ya excluidas
   por ADR-0023 no participan. Tampoco se compara una carpeta con un ancestro
   suyo dentro de la misma raíz: esa inclusión es consecuencia trivial de la
   jerarquía, no una relación entre copias.

3. **La generación de candidatos está acotada.** Un índice invertido asocia
   cada contenido con las carpetas que lo contienen. Un contenido presente en
   más de 32 carpetas se considera componente ubicuo y no genera pares. Los
   pares distintos se ordenan y se examinan como máximo 200 000; el número de
   candidatos omitidos se registra en el evento de análisis. El criterio
   predeterminado exige que los contenidos compartidos representen al menos la
   mitad de la unión de ambos conjuntos.

4. **Los contenedores pasa-through no duplican relaciones.** Un ancestro cuyo
   conjunto de contenidos es idéntico al de una carpeta descendiente (por
   ejemplo, `Backup/` que solo contiene `Backup/Expediente 77/`) se relaciona
   exactamente con las mismas carpetas que esa descendiente, y duplicaría cada
   una de sus relaciones. Antes de generar candidatos se suprimen esos
   ancestros y reporta únicamente la carpeta más profunda —la ubicación más
   específica—. Dentro de una raíz el conjunto del ancestro siempre contiene
   al del descendiente, de modo que la igualdad de cardinalidad implica
   igualdad de conjuntos. El número de contenedores suprimidos se registra en
   el evento de análisis (`pass_through_suppressed`).

5. **La selección es determinista entre rescaneos.** Cada carpeta se ordena por
   la clave estable `(source_root_id, relative_path)`, independiente del UUID
   `folder_id` generado en cada snapshot. Los pares se deduplican en un
   `BTreeSet`, el límite se aplica después de ordenarlos y la misma clave fija
   la orientación persistida A/B. Por tanto, dentro del mismo proyecto los
   límites no dependen del orden de lectura, de la semilla aleatoria de una
   tabla hash ni de los nuevos ids de carpeta de un rescaneo.

6. **Se emiten dos relaciones.** Tras excluir los clones exactos ya cubiertos
   por ADR-0023:

   - `PARTIAL_TREE_CLONE`: ambos lados conservan al menos un contenido que el
     otro no tiene;
   - `TREE_EMBEDDED`: todos los contenidos de una rama aparecen en la otra y
     esta contiene al menos uno adicional.

   Cada fila persiste los contenidos compartidos, los bytes compartidos y los
   **recuentos** exclusivos de A y B (`unique_a_files`, `unique_b_files`). M0.2
   no persiste la lista de rutas exclusivas; los recuentos son la evidencia
   mínima que impide presentar la relación como clon exacto.

7. **Son evidencia para conservación y revisión, no autorización de
   consolidación.** Una relación parcial crea una anomalía porque eliminar
   cualquiera de las ramas perdería contenido. Una relación embebida también
   se somete a revisión. Ninguna de las dos genera por sí sola una operación
   que omita una rama.

8. **Persistencia y superficie.** Las relaciones viven en `tree_relations`
   (migración `0009_tree_relations.sql`), se recomputan de forma idempotente por
   snapshot y se exponen en `dataforge report tree-relations`. El evento
   `STRUCTURE_ANALYZED` incluye los recuentos de relaciones y pares omitidos.

## Alternativas consideradas

- **Producto cartesiano de carpetas** — descartado por coste cuadrático y por
  amplificar componentes ubicuos.
- **Tratar cualquier contenido compartido como relación** — descartado: un
  logo o una plantilla común no demuestra que dos ramas sean copias.
- **Incluir pares ancestro/descendiente** — descartado: llenaría el informe de
  inclusiones inherentes al propio árbol.
- **Guardar solo una etiqueta sin evidencia exclusiva** — descartado: no
  permitiría demostrar por qué una rama parcial debe conservarse.
- **Consolidar automáticamente una rama embebida** — descartado: la identidad
  de contenido no demuestra que el contexto de esa rama sea prescindible.

## Consecuencias

- M0.2 identifica clones parciales y árboles embebidos de forma reproducible,
  con límites explícitos de tiempo y cardinalidad.
- Los límites aceptan falsos negativos: componentes demasiado frecuentes,
  carpetas triviales, pares por debajo del umbral o candidatos posteriores al
  techo no se materializan.
- `REPEATED_COMPONENT_ONLY` permanece en el vocabulario, pero no se persiste:
  por debajo del umbral no hay evidencia suficiente para distinguir relación
  de coincidencia.
- La implementación no construye un grafo semántico ni relaciona documentos
  por su significado. Es análisis estructural sobre identidades exactas de
  contenido.
- Condición de revisión: si el límite de 200 000 omite pares relevantes en
  colecciones reales, deberá introducirse particionado o paginación antes de
  elevarlo sin cota.
