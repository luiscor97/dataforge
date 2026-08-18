# La cola de revisión era la respuesta, no el problema

Durante tres runs traté `90_DataForge_Review` como un fallo: el sitio donde
acaba lo que el motor no supo colocar. Con 108.935 archivos y 247,6 GB dentro,
parecía la medida de lo que faltaba por construir.

Es al revés. **La cola de revisión es el mecanismo del criterio**, y estaba
vacía de decisiones porque nadie las había tomado.

```
Items    : 5334
Pending  : 5334
Decided  : 0
```

El trabajo original hizo exactamente esto: el detector marcaba, y una persona
decidía por clases enteras mirando la evidencia. Once días, nueve correcciones.
Lo que DataForge llama `review decide-batch` es esa conversación, escrita.

## Las cuatro clases, y qué dice la transcripción de cada una

El motor no solo agrupa: mide la clase antes de que nadie decida.

| clase | items | lo que el motor midió |
| --- | ---: | --- |
| `EMBEDDED_TREE` | 3.702 | **3702 de 3702 pares no contienen nada que le falte al otro lado; 0 contenidos únicos del lado contenido en toda la clase** |
| `PARTIAL_TREE_UNIQUE_CONTENT` | 902 | 0 de 902 pares; **10.395 contenidos únicos** |
| `EXTREME_PATH` | 724 | rutas cerca o más allá del límite de Windows |
| `review.backup-extension` | 6 | copias de respaldo |

Y la transcripción del trabajo original decide cada una, con sus palabras:

**Árbol embebido que es duplicado exacto** → *«Si el subárbol injertado es 100%
duplicado exacto: se marca como "árbol copiado accidentalmente" y no se
incorpora al repositorio documental principal»*. Se aparta; no se borra ni sube
al árbol firme.

**Árbol parcial con contenido único a los dos lados** → *«Si contiene mezcla de
duplicados y archivos únicos: los duplicados se descartan como copia
accidental, pero los únicos se aíslan en una carpeta de revisión»*. Se queda en
revisión a propósito: hay 10.395 contenidos únicos y eso es juicio humano.

**Ruta extrema** → *«150 rutas no eran visibles para las APIs antiguas sin
prefijo extendido. Se documentó como límite en vez de acortarlas en masa»*. El
archivo es válido; se conserva.

## El error que cometí dos veces

La primera vez decidí `COPY_ACTIVE` sobre los 3.702 árboles embebidos,
razonando que la política de duplicados los reduciría después. No podía
(§15.2), y 11.816 archivos sin nada propio entraron en el árbol entregado.

**Lo repetí.** Con el mismo razonamiento, sobre la misma clase. Esta vez lo
medí antes de enseñarlo:

```
Review:          108.935 → 91.639 archivos   (247,6 → 162,8 GB)
Copias totales:  157.321 → 157.321           sin cambio
Consolidados:        898 → 898               sin cambio
```

17.296 archivos y 84,8 GB no se consolidaron: **se mudaron de la carpeta de
revisión al árbol activo**. Exactamente el mismo daño, por el mismo camino.

Lo que hace `COPY_ACTIVE` es liberar la ocurrencia a enrutado normal. Para un
árbol embebido eso significa copiarlo entero al sitio firme, que es la única
cosa que el criterio dice que no hay que hacer con él. La decisión correcta es
`COPY_SEPARATED`: fuera del árbol activo, conservado, revisable — el
`03_Revision_Estructural` del trabajo original.

Anotado aquí porque el patrón importa más que el caso: **razoné sobre lo que la
política haría después en vez de mirar lo que la decisión hace ahora**, y lo
hice dos veces con las mismas 3.702 carpetas.

## Una consecuencia de diseño

Las decisiones son de solo añadir, y dos distintas sobre el mismo item cuentan
como conflicto y devuelven la ocurrencia a revisión. Es correcto para lo que
protege —nadie resuelve una contradicción humana por orden de fila— pero
significa que **cambiar de opinión exige un snapshot nuevo**. Con reuso de hash
eso ahora cuesta minutos en vez de cinco horas y media, así que es viable; sin
él, un error de criterio sobre una clase grande sería irreversible en la
práctica.
