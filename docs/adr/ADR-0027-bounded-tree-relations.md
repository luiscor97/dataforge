# ADR-0027 — Relaciones estructurales acotadas entre árboles

**Estado:** Aceptada
**Fecha:** 2026-07-16
**Relacionada con:** RFC-0001 §19.3, §19.4; ADR-0023

**Revisada:** 2026-07-17 para materializar `REPEATED_COMPONENT_ONLY` y
distinguir wrappers puros de árboles completos injertados dentro de sí mismos.

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
   por ADR-0023 no participan. Los pares ancestro/descendiente ordinarios se
   descartan antes de aplicar el límite: esa inclusión es consecuencia trivial
   de la jerarquía. Solo reentra un par jerárquico cuando la multiplicidad
   demuestra un auto-injerto según el punto 4.

3. **La selección y memoria de pares distintos están acotadas.** Un índice
   invertido asocia cada contenido con las carpetas que lo contienen. Un
   contenido presente en más de 32 carpetas se considera componente ubicuo y
   no genera pares; los demás producen como máximo 496 combinaciones por
   contenido. Los `content_id` y sus holders se recorren en orden estable. Un
   `BTreeSet` conserva como máximo 200 000 pares distintos y el recorrido se
   detiene al encontrar el primer candidato **nuevo** que ya no cabe; nunca se
   materializa la cola omitida para contarla. El evento publica únicamente el
   booleano honesto `candidate_cap_reached`.

   `max_pairs` no es un presupuesto de todas las consideraciones: dos carpetas
   que compartan muchos contenidos pueden ofrecer repetidamente el mismo par,
   que se deduplica sin consumir otra plaza. El trabajo total sigue el tamaño
   del índice y las combinaciones acotadas por contenido; imponer además un
   límite de CPU exigiría contar también duplicados y tendría otra semántica.
   El umbral predeterminado de la mitad de la unión separa clones
   parciales/árboles embebidos de una repetición de componente; no elimina la
   evidencia situada por debajo del umbral.

4. **La multiplicidad distingue wrappers de auto-injertos.** Un ancestro es
   pasa-through solo cuando su conjunto de contenidos **y su total de
   apariciones** coinciden con los de una descendiente (por ejemplo, `Backup/`
   que solo contiene `Backup/Expediente 77/`). Como las apariciones de la
   descendiente son un subconjunto multiconjunto de las del ancestro, la
   igualdad de totales demuestra que no existe ninguna aparición adicional
   fuera de ella; se suprime el wrapper y se reporta la ubicación más profunda.

   En cambio, `A/{f1,f2,A-copia/{f1,f2}}` presenta una segunda copia completa.
   La prueba se hace por `content_id`, no por el total agregado: para cada
   identidad, `apariciones_ancestro - apariciones_descendiente` debe ser al
   menos `apariciones_descendiente` (equivalente a ancestro ≥ 2 ×
   descendiente). Repetir solo un logo fuera de la descendiente no basta. El
   par probado se conserva como auto-injerto `REPEATED_COMPONENT_ONLY`; los
   wrappers que lo rodeen se suprimen para dejar los extremos concretos más
   profundos. El evento registra `pass_through_suppressed`.

5. **La selección es determinista entre rescaneos.** Los auto-injertos probados
   se ofrecen primero al presupuesto. Después se recorren `content_id`
   ordenados y, dentro de cada uno, carpetas ordenadas por la clave estable
   `(source_root_id, relative_path)`, independiente del UUID `folder_id`
   generado en cada snapshot. Los pares se deduplican en el `BTreeSet` acotado
   y la misma clave fija la orientación persistida A/B. Por tanto, dentro del
   mismo proyecto los límites no dependen del orden de lectura, de la semilla
   aleatoria de una tabla hash ni de los nuevos ids de carpeta de un rescaneo.

6. **Se emiten tres relaciones.** Un par no jerárquico sin ninguna identidad
   exclusiva en ninguno de los lados no encaja en esta taxonomía y se filtra
   antes del presupuesto. Una parte de esos casos serán clones Merkle exactos,
   ya cubiertos por ADR-0023; igualdad del conjunto de `content_id` por sí sola
   no prueba igualdad Merkle de nombres, estructura ni multiplicidad.

   - `PARTIAL_TREE_CLONE`: ambos lados conservan al menos un contenido que el
     otro no tiene;
   - `TREE_EMBEDDED`: todos los contenidos de una rama aparecen en la otra y
     esta contiene al menos uno adicional;
   - `REPEATED_COMPONENT_ONLY`: el solapamiento queda bajo el umbral (logo,
     plantilla u otro componente legítimamente compartido), o la multiplicidad
     prueba que el conjunto completo de una rama aparece de nuevo dentro de su
     propio ancestro. No se presenta como clon.

   Cada fila persiste los contenidos compartidos, los bytes compartidos y los
   **recuentos** exclusivos de A y B (`unique_a_files`, `unique_b_files`). M0.2
   no persiste la lista de rutas exclusivas; los recuentos son la evidencia
   mínima que impide presentar la relación como clon exacto.

7. **Son evidencia para conservación y revisión, no autorización de
   consolidación.** Una relación parcial crea una anomalía porque eliminar
   cualquiera de las ramas perdería contenido. Una relación embebida también
   se somete a revisión. La repetición de componente queda como evidencia
   informativa y no crea anomalía ni revisión. Ninguna relación genera por sí
   sola una operación que omita una rama.

8. **Persistencia y superficie.** Las relaciones viven en `tree_relations`
   (migración `0009_tree_relations.sql`), se recomputan de forma idempotente por
   snapshot y se exponen en `dataforge report tree-relations`. El evento
   `STRUCTURE_ANALYZED` incluye los recuentos de relaciones y
   `candidate_cap_reached`; no afirma conocer cuántos pares quedaron sin
   generar. El mismo booleano queda sellado en `analysis_completions` y se
   muestra en el resultado de `analyze`, diagnósticos, CLI y desktop para no
   presentar el análisis estructural como exhaustivo cuando alcanzó el techo.

## Alternativas consideradas

- **Producto cartesiano de carpetas** — descartado por coste cuadrático y por
  amplificar componentes ubicuos.
- **Tratar cualquier contenido compartido como clon parcial o árbol embebido**
  — descartado: un logo o una plantilla común no demuestra que dos ramas sean
  copias; se conserva con una etiqueta informativa distinta.
- **Incluir todos los pares ancestro/descendiente** — descartado: llenaría el
  informe de inclusiones inherentes al propio árbol. La excepción acotada exige
  igualdad de conjunto y apariciones adicionales demostrables.
- **Guardar solo una etiqueta sin evidencia exclusiva** — descartado: no
  permitiría demostrar por qué una rama parcial debe conservarse.
- **Consolidar automáticamente una rama embebida** — descartado: la identidad
  de contenido no demuestra que el contexto de esa rama sea prescindible.

## Consecuencias

- M0.2 identifica clones parciales, árboles embebidos, componentes repetidos y
  auto-injertos de forma reproducible. La memoria del conjunto de pares
  distintos y las relaciones finalmente examinadas quedan limitadas por
  `max_pairs`. Esa cota no cubre todo el análisis: el roll-up mantiene set y
  multiplicidad por identidad para cada ancestro y ocupa
  O(suma de profundidades de las apariciones), que puede ser cuadrático en una
  jerarquía adversarialmente profunda; el prepass de auto-injertos también
  recorre ancestros. El índice y la reconsideración deduplicada añaden hasta
  496 combinaciones por contenido no ubicuo.
- Los límites aceptan falsos negativos: componentes demasiado frecuentes,
  carpetas triviales o candidatos posteriores al techo no se materializan.
- `REPEATED_COMPONENT_ONLY` sí se persiste, dentro del mismo techo determinista,
  para que una coincidencia legítima o un auto-injerto no se confundan con un
  clon accionable.
- La implementación no construye un grafo semántico ni relaciona documentos
  por su significado. Es análisis estructural sobre identidades exactas de
  contenido.
- Condición de revisión: si el límite de 200 000 omite pares relevantes en
  colecciones reales, deberá introducirse particionado o paginación antes de
  elevarlo sin cota.
