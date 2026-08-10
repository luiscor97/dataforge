# Las tres decisiones bloqueantes, medidas

**Fecha:** 2026-08-10 · **Estado:** material para decidir, no decisión tomada

Las tres cosas que bloquean el siguiente bloque de la 2.0 llevaban tiempo
descritas de memoria. Aquí están medidas sobre el corpus real y sobre la base
de datos del trabajo original, para que la decisión sea sobre datos.

> Los datos viven en `D:\DataForge_Audit_Work\dataforge.sqlite` (trabajo
> original) y `D:\Discolocal-proyecto` (reejecución con la 1.0). Este documento
> no reproduce rutas ni nombres de cliente: solo formas y recuentos.

## Decisión 1 — la precedencia de ADR-0045

### La pregunta no es «aprobar o rechazar»

ADR-0045 está **parcialmente implementada y es insuficiente como está
escrita**, y ella misma lo dice. `DuplicateKind::ContainedTreeReplica` funciona
y **no tiene ningún efecto observable**, porque una relación `TREE_EMBEDDED`
levanta un ítem de revisión, y una revisión decidida se convierte en
recomendación, y una recomendación gana a la consolidación de duplicados.

Lo que hay que decidir es esa precedencia. La ADR plantea tres opciones; la 2
es «que `TREE_EMBEDDED` no genere ítem de revisión cuando el lado contenido no
tiene contenido propio».

### Lo primero que hay que saber: esa condición es siempre verdadera

El `CHECK` de la migración 0009 lo impone:

```sql
CHECK (
    relationship <> 'TREE_EMBEDDED'
    OR (contained = 'A' AND unique_a_files = 0)
    OR (contained = 'B' AND unique_b_files = 0)
)
```

Toda relación `TREE_EMBEDDED` ya cumple `unique_files = 0` en el lado
contenido. Así que la opción 2, redactada como condicional, significa en
realidad: **`TREE_EMBEDDED` deja de generar ítem de revisión, sin condición.**
Conviene decidirla sabiendo eso.

### Lo segundo: qué son de verdad esas carpetas

Medido sobre las 3.702 relaciones `TREE_EMBEDDED` del corpus:

| | |
| --- | --- |
| Relaciones `TREE_EMBEDDED` | 3.702 |
| Lado contenido **físicamente dentro** del otro | **0 (0%)** |
| Lado contenido en **otra ruta** | 3.702 (100%) |
| Pares mutuos (X⊆Y e Y⊆X) | **0** |
| Carpetas que aparecen en **ambos** lados | 266 |

Ninguna carpeta contenida está dentro de la otra. Son **el mismo árbol en dos
rutas distintas** — el caso de árbol injertado. Así que «colapsar» no es quitar
una carpeta anidada: es **decidir qué ruta es la canónica**.

Y no hay ciclos, lo cual importa: la dirección de cada par es inequívoca. Pero
266 carpetas están en ambos lados, así que existen cadenas A⊇B⊇C y el filtro de
maximales que la ADR ya aplica no es un detalle.

### Lo tercero: la elección que haría el motor es la correcta

Aquí me equivoqué al juzgar por nombres de carpeta antes de medir. Los nombres
del lado contenido parecen asuntos —periciales, desahucios, clientes— y solo 41
de 3.702 (1,1%) suenan a copia, lo que sugiere que colapsar perdería
significado.

Medido, es al revés. Sobre las 27.692 decisiones comparables del trabajo
original:

| | |
| --- | --- |
| Se quedó la ruta **más corta** | 17.730 (64,0%) |
| Se quedó la ruta **más profunda** | 2.531 (9,1%) |
| Misma profundidad | 7.431 (26,8%) |
| **La ruta que se queda es cola de una descartada** | **22.190 (80,1%)** |

Ese 80,1% es la forma del injerto: la copia descartada llevaba un prefijo que
la conservada no tiene. Y al mirar los casos, **el prefijo extra no es
contexto, es el injerto**: una carpeta de curso que absorbió una copia de otra
carpeta entera, una carpeta de cliente que se tragó un directorio de ayuda de
una aplicación y el instalador de un OCR.

Es decir: la ruta más corta **sí** es la canónica en este corpus, y el
representante que elige el motor —siempre fuera del subárbol contenido— cae del
lado bueno.

### Qué queda sin medir

El 9,1% donde se conservó la ruta más profunda, y el 19,9% que no encaja en la
forma de cola. Son ~7.700 grupos. No se sabe si ahí la elección es buena.

### Recomendación

**Opción 2, pero después de `grafted_tree_report`.** La evidencia apoya que la
elección automática es correcta en la forma dominante, y eso hace la opción 2
razonable. Lo que todavía no existe es la herramienta que lo *demuestre* caso a
caso — la ruta canónica probable y el reparto 99,1/0,9. Adoptar la opción 2
antes sería tomar la decisión correcta sin poder enseñarla, que en un archivo
probatorio es la mitad del trabajo.

## Decisión 2 — el reencuadre de M2.3

Sin novedad respecto a lo ya escrito, y la evidencia lo refuerza: el trabajo
original no terminó con un veredicto por archivo, terminó con marcadores y
raíces. Las 47.982 decisiones que dejó **no son juicios semánticos**: son
elecciones de ruta canónica dentro de grupos de duplicados, tomadas por una
fórmula.

Eso apoya el trío `propose_profile` / `validate_profile` / `apply_profile`:
un perfil pequeño y auditable que un humano lee, frente a 158.219 juicios que
nadie puede revisar. Necesita su ADR antes de escribirse.

## Decisión 3 — auditar una muestra del criterio humano

### La premisa era falsa

La tarea decía «auditar una muestra estratificada del criterio humano». Medido:

| | |
| --- | --- |
| Grupos decididos | 47.982 |
| **Que tocó un humano** | **0 (0,0%)** |
| Que un humano cambió | 0 |
| `retention_action` distinto de `keep` | 0 |

**No hay criterio humano que auditar.** Las 47.982 las decidió la fórmula
`depth −12; path_length −3.2; oldest_mtime +15`, y nadie revisó ninguna. Lo que
hay que auditar es el criterio **automático**, que es una tarea distinta y más
urgente: es la primera vez que se comprueba.

Y la confianza no ayuda a priorizar, porque casi todo salió confiado:

| Banda de `auto_score` | Grupos | |
| --- | ---: | ---: |
| ≥100 | 22.006 | 45,9% |
| 75–100 | 25.147 | 52,4% |
| 50–75 | 815 | 1,7% |
| 25–50 | 14 | 0,0% |

### La muestra ya está hecha

134 grupos estratificados por banda de confianza, con semilla fija (`20260810`)
para que dos personas auditen las mismas filas. Cada fila trae la ruta que se
quedó, hasta tres descartadas, el score y el motivo, y una columna vacía para
el veredicto.

Un hallazgo al construirla: la banda de **menor** confianza no es la
informativa. Los 14 grupos de 25–50 son todos copias de un backup de GPS —
material que además se excluiría por no ser jurídico. La fórmula duda donde da
igual.

Lo informativo es el 9,1% en que conservó la ruta más profunda, que la
estratificación por score no aísla. **La muestra debería estratificarse por
forma de la decisión, no por confianza** — y eso ya se puede hacer con la
consulta de arriba.

## Qué se lleva cada decisión

| | Bloquea a | Necesita |
| --- | --- | --- |
| 1 · precedencia ADR-0045 | los 118,9 GB de M2.3 | `grafted_tree_report` antes |
| 2 · reencuadre M2.3 | el trío de perfil | su propia ADR |
| 3 · auditar el criterio | calibrar umbrales | reestratificar y juzgar 134 filas |

Ninguna de las tres la puede tomar el motor, y ninguna necesita ya que nadie
recuerde de memoria cómo era el corpus.
