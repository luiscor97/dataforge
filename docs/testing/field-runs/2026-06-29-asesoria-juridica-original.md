# 2026-06-29 → 2026-07-10 — El trabajo original sobre el archivo jurídico

**Herramienta:** ninguna. Codex conduciendo scripts Python ad-hoc.
**Equipo:** Windows, origen y destino en el mismo disco `D:`.
**Fuente de los datos:** transcripción completa de la sesión (49 turnos,
2026-06-29 → 2026-07-29) más medición directa del corpus.

**Esto no es una ejecución de DataForge.** Es el trabajo que DataForge existe
para reproducir. Se registra aquí porque sus cifras son la definición de hecho
de la 2.0 y hasta ahora solo se citaban de memoria: ROADMAP-2.0 dice
«reproducir el resultado que hoy solo se alcanzó con scripts y criterio humano
a lo largo de diez días» sin decir cuál era ese resultado.

## Corpus

Archivo documental completo de una asesoría jurídica, con material personal y
de ocio mezclado. Once días de trabajo, del 29 de junio al 10 de julio.

| | Citado en ROADMAP-2.0 | Medido hoy sobre `D:\Discolocal` |
| --- | --- | --- |
| Archivos | 158.219 | **155.906** |
| Bytes | 443,9 GB | **443,4 GB** |
| Carpetas | 36.459 | **36.455** |

La diferencia (2.313 archivos, 0,5 GB) no está explicada. Puede ser una copia
tomada en momento distinto, o el recuento del original frente al de una copia
parcial. **Ninguna cifra de este documento debe tratarse como exacta al
archivo mientras eso no se aclare**; las órdenes de magnitud sí valen.

## Resultado final

| | |
| --- | --- |
| Archivos de contenido archivístico | **29.228** |
| Archivos físicos (con informe y manifiesto) | **29.240** |
| Tamaño total | **150,57 GB** (138,2 GB jurídico + 12,37 GB separado) |
| Grupos de duplicados exactos | **0** |
| Rutas del manifiesto ausentes | **0** |
| Música/ocio dentro de la zona operativa | **0** |
| Origen modificado | **ninguno** |

**De 443,4 GB a 150,57 GB. De ~156.000 archivos a 29.228.**

Esa reducción no es sobre todo deduplicación: es exclusión de material no
jurídico más deduplicación más reubicación semántica. Distinguir las tres es
importante, porque DataForge 1.0 solo sabe hacer la segunda — y de forma muy
acotada (ver §«Qué mide esto de la 1.0»).

## Estructura que emergió

Nadie la diseñó por adelantado; salió de discutir casos reales.

```
05_Asesoria_Juridica_v2/
  00_INFORME_DE_ORGANIZACION/     informe al asesor, PDF, CSV de trazabilidad, manifiesto
  01_Asesoria_Juridica/           la zona operativa, por cliente/asunto
  02_Correos/                     luego reubicados dentro de su asunto a petición del usuario
  03_Periciales/ 04_Periciales_Caligraficas/
  05_RECURSOS_JURIDICOS_Y_FORMATIVOS/
  90_CONTENIDO_SEPARADO/          613 contenidos, 12,37 GB: 549 audios + 35 vídeos musicales,
                                  11 películas/series, personal, técnico/software, publicidad
  90_Revision_Origen_Mixto/
  91_Revision_Periciales_Origen_Mixto/    ← el único límite declarado
  92_Revision_Estructural/
```

Dos cosas que el usuario corrigió a mano y que ninguna heurística acertó
sola: los correos debían acabar **dentro** de su asunto, no en un contenedor
propio; y una carpeta `CORREOS` bajo un cliente contenía en realidad correos
*a* cada agente comercial, que iban a la carpeta de cada uno.

## Los problemas reales, en palabras del usuario

1. **Árboles injertados.** «en la carpeta /agentes comerciales se pueden
   encontrar perdidos en algún directorio el mismo árbol de carpetas fruto de
   arrastrar y copiar mal». Ejemplo literal:
   `/agentes comerciales/<persona>/agentes comerciales/`.
2. **Mismo nombre de archivo en asuntos distintos.** En periciales
   caligráficas, las fotos se guardan desde la SD en *fotos*, *descargas* o
   *escritorio* (las tres o ninguna) y luego se renombran a «dubitada» e
   «indubitada». El mismo nombre existe en varios asuntos y **no significa el
   mismo contenido**. De ahí el criterio explícito: *no fusionar ni mover por
   nombre en asuntos periciales*.
3. **Ocio mezclado.** Música y películas dentro del archivo profesional,
   detectables por tamaño pero no por ubicación.
4. **Vídeo formativo que parece ocio.** «esos vídeos no los considero
   películas, son vídeos cortos de un curso». El tamaño no basta como señal.

## Lo que hay que aprender de aquí

**El criterio de aceptación no fue técnico.** Fue: *«que el asesor no tenga
desconfianza porque se haya podido perder material alguno»*. Por eso el
entregable acabó siendo informe + PDF + CSV de trazabilidad + manifiesto
SHA-256 dentro de la propia carpeta. Un resultado correcto que no se puede
demostrar no sirve.

**El único límite que quedó abierto fue criterio humano**, en periciales de
origen mixto — señalizado en `91_`, no resuelto. Es exactamente el fallback a
`revisar/` que RFC-0002 propone, descubierto por necesidad antes de diseñarse.

**Fue reanudable por hash, no por memoria.** Una ejecución se cortó por un
error de impresión Unicode en un nombre de archivo —consola, no datos— y el
relanzamiento comparó SHA-256 antes de copiar para no duplicar. Es la misma
propiedad que el executor de DataForge tiene por diseño.

**Rutas largas.** 150 rutas no eran visibles para las APIs antiguas sin
prefijo extendido. Se documentó como límite en vez de acortarlas en masa.

## Qué mide esto de la 1.0

Sobre este mismo corpus, `consolidation_savings.rs` y las mediciones de
`hallazgos-2026-08-01` dicen que de 239,7 GB de redundancia solo **5,45 GB**
son alcanzables por cualquier política de la 1.0, porque de 28.537 conjuntos
de duplicados solo 625 tienen todas sus copias en una misma carpeta y §15.2
prohíbe inferir redundancia en el resto.

Contrastado con lo que este trabajo logró —443,4 GB → 150,57 GB— el hueco
queda dimensionado: **la 1.0 no puede acercarse, y no por falta de política
sino por falta de clasificación.** Es la razón de que M2.3 sea precondición y
no adorno.

## El rastro de evidencia existe, y es mejor de lo esperado

`D:\DataForge_Audit_Work\` conserva la auditoría completa del trabajo. No es
un resumen: es el conjunto etiquetado de decisiones, archivo a archivo.

| Artefacto | Contenido |
| --- | --- |
| `dataforge.sqlite` | **156.962 archivos** inventariados, **47.982 decisiones**, 2.281 medios |
| `asesoria_juridica/manifest_asesoria_juridica_v2.csv` | 47.982 filas: `category, reason, source_abs_path, dest_abs_path, size, mtime_ns, sha256` |
| `estructura/arboles_injertados_detalle.csv` | **135.378 filas** de árbol injertado con ruta canónica probable |
| `estructura/periciales_mismo_nombre_hash_distinto.csv` | 1.463 filas |
| `estructura/periciales_imagenes_compartidas_entre_asuntos.csv` | 7.276 filas |
| `media_clasificada.csv` | 2.281 medios con `media_type` y `reason` |

**Esto es exactamente la muestra de criterio humano que ROADMAP-2.0 declara
pendiente de auditar.** Ya no hay que construirla: hay que leerla.

### Las decisiones, por categoría

| Categoría | Archivos | Razón dominante |
| --- | ---: | --- |
| `excluido_no_juridico` | 19.413 | «sin señales suficientes» (11.242), «software/técnico» (8.171) |
| `asesoria_main` | 12.426 | «raíz jurídica reconocida» |
| `correos` | 7.873 | «correo o contenedor de correo» |
| `revision_origen_mixto` | 5.576 | «vocabulario jurídico fuera de raíz principal» (4.433) |
| `periciales` | 2.331 | «pericial/fotos/asunto caligráfico» |
| `pericial_revision_origen_mixto` | 176 | «dentro de raíz mixta o copia arrastrada» |
| `soporte_juridico` | 160 | «archivo técnico dentro de raíz jurídica» |
| `revision_estructural` | 27 | «contenido único en contexto sospechoso» |

**El 40 % de las decisiones fue excluir.** Y la razón mayoritaria —«sin
señales suficientes»— es un juicio de ausencia, no de presencia: exactamente
el tipo que una regla determinista puede formular y un modelo no debería
inventar.

### Árboles injertados: el 99,1 % se resuelve solo

135.378 archivos afectados en **124 prefijos injertados distintos**:

| Situación | Archivos | |
| --- | ---: | --- |
| `canonical_path_same_hash` | 130.165 | 96,1 % — está en su ruta canónica con el mismo contenido |
| `hash_elsewhere_outside_prefix` | 3.977 | 2,9 % — el contenido existe fuera del injerto |
| `unique_hash_not_elsewhere` | **817** | 0,6 % — **contenido único dentro del injerto** |
| `canonical_path_hash_diff` | **419** | 0,3 % — misma ruta canónica, **contenido distinto** |

Los dos últimos son los que no se pueden tocar sin criterio: 1.236 archivos,
el **0,9 %**. Es literalmente la taxonomía que RFC-0002 llama
«clean-replacement=auto vs unique-in-suspicious-context=review», descubierta
por necesidad y ahora cuantificada. **Un umbral de auto-colocación para
árboles injertados no es una suposición: son 99,1 / 0,9.**

### Periciales: la prueba de las dos reglas, en el mismo archivo

**Mismo nombre, contenido distinto.** 106 nombres afectados. El peor:

```
00000001.JPG  → 19 hashes distintos en 6 asuntos
DUB-1.TIF     →  8 hashes distintos en 6 asuntos
DSC_0013.JPG  →  8 hashes distintos en 7 asuntos
```

Deduplicar por nombre habría fusionado diecinueve imágenes distintas de seis
periciales caligráficas. En un peritaje, eso no es desorden: es destruir
prueba.

**Mismo contenido, asuntos distintos.** 678 hashes únicos aparecen en más de
un asunto (7.276 apariciones). Aquí la regla es la contraria: **no
consolidar**, porque cada expediente debe sostenerse solo como unidad
probatoria (regla 9, §15.3 `ACROSS_PROTECTED_CONTEXTS`).

Las dos reglas que DataForge ya afirma por diseño, con datos reales que
demuestran que ambos casos existen a escala **en el mismo archivo**.

## Los criterios, en forma ejecutable (extraídos 2026-08-16)

Lo anterior explica el trabajo. Esta sección existe para que un agente pueda
**actuar** sin volver a leer 1.528 mensajes: cada criterio con su cita, la
acción que le corresponde en el motor, y si hoy se puede o no.

La regla que gobierna la tabla: **se decide lo que el registro decide.** Lo que
el trabajo original no resolvió se deja pendiente, no se inventa por simetría.

| Criterio | Cita | Acción en el motor | Hoy |
| --- | --- | --- | --- |
| Un representante por grupo de clones exactos, elegido por profundidad, nombre, ruta y antigüedad | turno 736 | `plan create --duplicate-policy CONSOLIDATE_ALL` | intención sí, efecto **no** |
| El destino conserva la estructura relativa del representante | turno 980 | comportamiento por defecto del planner | sí |
| Un archivo duplicado cuyo uso está dentro de un expediente **no** se consolida | turno 1302 | regla 9, fronteras protegidas | **no dispara** |
| Media, entretenimiento y material técnico se aíslan, no se borran | turno 272 | perfil + `95_Separated` | sí |
| Árboles embebidos y parciales: se colocan ambos lados | turnos 736 + 1302 | `review decide-batch` → `COPY_ACTIVE` | sí |
| Rutas de 240+ caracteres se conservan | informe final | `review decide-batch` → `COPY_ACTIVE` | sí |
| Estructura por materia, no espejo del origen | estructura entregada | — | **no**, M2.7 |
| Reglas de conservación por antigüedad | *«no apliqué reglas RGPD automáticas al no tener una política concreta»* (turno 1171) | — | **no se decidió** |

### La pieza que faltaba aislar: el perfil vacío

El criterio del turno 1302 es, palabra por palabra, la regla 9 de RFC-0001. El
motor **la tiene implementada y nunca la dispara**: el run de campo reportó
`Protected bounds: 0`, porque el perfil `generic` no marca ninguna carpeta como
frontera protegida.

Así que las dos mitades del criterio humano faltan a la vez y por causas
distintas: no puede consolidar —§15.2 se lo prohíbe sin clasificación— y no
tiene nada que proteger —el perfil está vacío—. No es un fallo de código. Es un
perfil que nadie ha escrito, y es lo que un perfil `asesoria-juridica` con
fronteras protegidas y categorías de material no documental resolvería
(ADR-0026: los perfiles se compilan, así que es un cambio con su ADR).

### El banco: puntuar un criterio sin ejecutar nada

El trabajo original probaba criterios **ejecutando y corrigiendo**: diez días,
nueve correcciones, cada una descubierta al ver una entrega entera. DataForge
estaba heredando ese método — llegaron a proponerse runs de quince horas para
ver qué forma salía.

No hace falta. Ese trabajo dejó su resultado con identidad de contenido, así
que un criterio candidato se aplica sobre el plan y se cruza contra la verdad
**en segundos**, sin copiar un byte.

```
antes : criterio → 15 h de run → mirar la forma → corregir
ahora : criterio → cruce contra el manifiesto → % de acuerdo → iterar
```

**Cobertura del plan construido con los criterios de la tabla anterior**
(decisiones del registro + `CONSOLIDATE_ALL`):

| | Contenidos |
| --- | ---: |
| Entregado por la persona | 29.239 |
| Que el plan coloca en el árbol limpio | **29.227 — 100,0%** |
| Que caen en revisión | 1 |
| Que no coloca | 11 — *todos del informe que ella escribió al final* |
| Que coloca de más | 20.572 (180,3 GB) |

**El desvío es solo por exceso, nunca por omisión.** Para un motor probatorio
ésa es la única dirección aceptable del error, y ahora está medida en vez de
supuesta.

**Cuatro criterios candidatos, evaluados en diez minutos:**

| Regla | Árbol limpio | Cubre | Sobran |
| --- | ---: | ---: | ---: |
| ninguna (el plan tal cual) | 49.799 | 100,0% | 20.572 |
| apartar software por extensión | 47.562 | 99,1% | 18.576 |
| apartar fotografía por extensión | 41.452 | **89,8%** | 15.205 |
| apartar volcaderos de foto **por carpeta** | 45.379 | **99,5%** | 16.279 |

El resultado que vale es el negativo. La regla obvia —apartar fotografía por
extensión, que es lo que el turno 272 parece decir— **destruye el 10,2% del
entregable**, porque las fotos periciales *son* la prueba. La misma intención
aplicada por ubicación conserva el 99,5%.

**La señal que separa material documental de no documental no es el tipo de
archivo: es dónde está.** Eso es lo que M2.3 tiene que dar, y deja de ser un
argumento para ser una medida.

### El criterio de representante, recuperado y contrastado

`decisions` guarda las 47.982 elecciones con su puntuación y **cero
intervenciones manuales**: no fue un juicio caso a caso, fue una fórmula que
nadie tuvo que corregir ni una vez.

```
group_key  : sha256:000290ed…
auto_score : 105.9
auto_reason: "depth -8; path_length -1.1; oldest_mtime +15"
```

| Señal | Trabajo original | DataForge |
| --- | --- | --- |
| profundidad | −4 por nivel | sí |
| longitud de ruta | penalización continua | no |
| **antigüedad del contenido** | **+15** | **no la mira** |
| carpeta genérica | no la usa | sí, pero inerte con el perfil vacío |
| marcador de copia en el nombre | no la usa | sí |

Comparando elección con elección sobre los grupos que ambos ven:

> **20.442 de 27.861 — 73,4% de acuerdo.**

Las discrepancias tienen forma reconocible. El trabajo original prefiere la
importación original de la cámara; DataForge, la copia dentro del proyecto:

```
campo     : imagenes\100d5600\dsc_0908.jpg
dataforge : fotos villar obra\garaje\dsc_0908.jpg
```

Que es exactamente lo que produce un bonus de antigüedad que uno tiene y el
otro no.

Ese 73,4% es el número que el hito del criterio tiene que subir, y ahora
existe.

### Las fuentes, por orden de fuerza

1. **`MANIFIESTO_FINAL_SHA256.csv`** — 29.240 filas: `rel_path, category, size,
   sha256, path_length, hash_source`. El entregable entero con identidad de
   contenido. **Es la fuente más fuerte y la que más tarde se usó**: hubo un
   contraste que rehasheó 86,67 GB durante 26 minutos para reconstruir a mano
   un subconjunto de lo que este fichero ya tenía. Cruzarlo contra
   `content_objects` es cuestión de segundos.
2. **`INFORME_FINAL_ORDENACION_...md`** — los criterios como se entregaron, y
   los recuentos que permiten dimensionar el hueco.
3. **La transcripción** — el *porqué* de cada criterio, que es lo que permite
   aplicarlo a un caso nuevo en vez de copiarlo.

### La validación que da confianza en todo lo anterior

| | Documentos | Únicos |
| --- | ---: | ---: |
| Trabajo original | 154.681 no multimedia | **47.982 representantes** |
| DataForge sobre el mismo origen | 158.219 archivos | **49.916 contenidos distintos** |

Coinciden al 4%. El recuento de representantes de una persona y el de
contenidos distintos del motor son la misma cifra medida de dos maneras. Lo que
separa 48.000 de 158.000 en el árbol limpio no es detección: es permiso para
consolidar, que es M2.3.

## Qué sigue faltando

- ~~Un extracto de criterios sin nombres.~~ **Hecho**: la sección anterior.
- **El informe final al asesor** (`INFORME_FINAL_ORDENACION_...docx` y PDF)
  contiene datos del cliente: no debe entrar tal cual.
- **Conciliar tres recuentos del mismo corpus**: 158.219 (ROADMAP-2.0),
  156.962 (base del trabajo original), 155.906 (medido hoy en disco). Momentos
  de medición distintos es la explicación probable, no la comprobada.
- Los CSV son grandes (84 MB el de árboles injertados) y contienen rutas
  reales. **No se versionan aquí**; lo que se versiona es este análisis. Si
  hace falta evidencia en el repo, van agregados sin rutas.

Sin nombres de cliente: este documento describe formas y recuentos a
propósito. Las rutas y nombres reales viven en el disco, no aquí.
