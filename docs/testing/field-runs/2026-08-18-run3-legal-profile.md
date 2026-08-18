# 2026-08-18 — Run 3, el primero con el perfil correcto

**Origen:** `D:\Discolocal`, 158.219 archivos, 443,9 GB. No modificado.
**Perfil:** `legal` — los dos runs anteriores corrieron con `generic`.
**Duración:** escaneo 5m19s, hash 5h33m (158.219 archivos, 0 errores), análisis
100s, dos planes completos 7m.

## Lo que este run existía para responder

| | run 1 y 2 | run 3 |
| --- | ---: | ---: |
| `Protected bounds` | **0** | **255** |
| `Profile fitness` | (no existía) | *no shipped profile would protect more* |
| `Grafted roots` | (no existía) | **183** |

Los 255 coinciden **exactamente** con la predicción hecha por separado, midiendo
los 36.458 nombres de carpeta del archivo con el `Profile::classify()` real
antes de lanzar nada. Predicción y motor cuadran al archivo, lo que dice que la
señal mide lo que dice medir.

La señal de idoneidad calla, que es lo correcto: con `legal` puesto, ningún otro
perfil publicado protegería más. El mecanismo avisa cuando hace falta y se
calla cuando no.

**Las tres preguntas salen que sí.** Y el resultado sigue sin parecerse al
humano.

## El resultado

| | humano, 11 días | run 3 |
| --- | ---: | ---: |
| Archivos entregados | **28.615** | **157.286** |
| Tamaño | **138,2 GB** | **435,2 GB** |
| Duplicados exactos consolidados | 106.699 | **933** |
| A revisión | — | 108.935 archivos, 247,6 GB |

El motor copia el disco entero, ordenado y verificado. El humano entregó una
quinta parte.

## De dónde sale cada gigabyte de diferencia

La cuenta cierra, y en tres piezas de tamaños muy distintos.

**1. Deduplicación global — 106.699 archivos.** El inventario tiene 28.537
conjuntos de duplicados exactos con **108.303 archivos redundantes y 257 GB**.
`CONSOLIDATE_ALL` alcanza **933**. No es un fallo: es RFC-0001 §15.2, que
prohíbe inferir que una copia sobra sin clasificación de contexto, así que solo
se consolidan los conjuntos cuyas copias viven todas en la misma carpeta.

El trabajo humano hizo exactamente lo que §15.2 prohíbe: **un representante por
contenido, global, elegido por profundidad, nombre, ruta y antigüedad.** Sin
restricción de contexto. Es la regla 2 de su informe final al asesor.

**2. Exclusión de lo no jurídico — 19.413 archivos, 68,1 GB.** El 40 % de sus
decisiones fue excluir, con dos reglas:

- «sin señales suficientes de asesoría jurídica» — 11.242 archivos
- «software/técnico/no jurídico» — 8.171 archivos

El motor no tiene ninguna regla de este tipo. Nada se excluye por no ser
material del dominio.

**3. Cuarentena de media previa al hash — 136,7 GB.** 2.281 archivos de música y
vídeo salieron del universo documental *antes* de calcular una huella, y solo
volvieron los que resultaron ser jurídicos o formativos. El motor los hashea
todos y los copia todos.

### El criterio completo son diez reglas

Sobre los 47.982 representantes, el reparto entero de decisiones:

| archivos | categoría | razón |
| ---: | --- | --- |
| 12.426 | `asesoria_main` | raíz jurídica reconocida |
| 11.242 | `excluido_no_juridico` | sin señales suficientes de asesoría jurídica |
| 8.171 | `excluido_no_juridico` | software/técnico/no jurídico |
| 7.873 | `correos` | correo o contenedor de correo |
| 4.433 | `revision_origen_mixto` | vocabulario jurídico fuera de raíz principal |
| 2.331 | `periciales` | pericial/fotos/asunto caligráfico |
| 1.143 | `revision_origen_mixto` | documento jurídico dentro de raíz mixta |
| 176 | `pericial_revision_origen_mixto` | pericial dentro de raíz mixta o copia arrastrada |
| 160 | `soporte_juridico` | archivo técnico dentro de raíz jurídica |
| 27 | `revision_estructural` | contenido único en contexto sospechoso |

Diez reglas y una razón escrita por archivo. Eso es todo lo que separa 443,9 GB
de 138,2 GB.

## Lo que esto significa para el roadmap

El hueco **no era el perfil**. El perfil era una pieza, está puesta, y se ve
funcionando: 255 fronteras protegidas donde antes había cero.

Lo que queda es de otro orden. La pieza mayor —106.699 archivos— está bloqueada
por una **regla**, no por una función que falte. §15.2 existe por una razón
buena: decidir que una copia en otra carpeta sobra, sin saber qué es esa
carpeta, es cómo se pierde material. Y la clasificación es exactamente la
precondición que lo desbloquea, que es lo que M2.3 dice desde el principio.

La novedad no es la dirección, es el número: **M2.3 vale 106.699 archivos y
257 GB.** Sin ella el motor no puede acercarse, y con ella las otras dos piezas
—exclusión de dominio y cuarentena de media— son reglas declarativas de tamaño
modesto comparadas con esta.

## Un problema propio, encontrado por el propio run

`plan validate` **rechazó** el plan consolidante con 28 problemas. Es el
invariante de las cicatrices de arrastre haciendo su trabajo: se niega a colocar
en el árbol activo ramas que no tienen nada propio.

Pero **13 de los 28 son ruido técnico o música**:

```
obs-studio\data\obs-studio
MUSICA\Frank Sinatra\Frank Sinatra
FORMULARIOS FUNDACION Y MECENAZGO\rtf\9\9
DESCARGAS\WC6605ScanDriverWithInstaller\WC6605ScanDriverWithInstaller
```

El invariante tiene razón en cada uno y aun así el efecto es malo: **bloquea el
plan entero por `node_modules` y por Frank Sinatra**, y el operador no puede
avanzar por algo que no es del expediente. Un invariante correcto que no se
puede accionar entrena a saltárselo, que es el mismo defecto que la advertencia
de `plan tree` — con la diferencia de que este no deja pasar.

Además, `FORMULARIOS...\rtf\9\9` es justo el caso que el colapso de componentes
adyacentes deja pasar a propósito, por numérico, para no romper `2020\2020`. Las
dos decisiones son defendibles por separado y juntas dejan al operador atascado.
Queda anotado sin arreglar: medir primero cuántos de los 28 sobreviven al
colapso, y decidir después.

## Nota de método

El guion de este run traía un `status` que no es un subcomando, y la etapa 5
salió con código 2 por eso. Fallo del guion, no del motor. Se registra porque un
error de guion que parece un fallo de producto es la clase de ruido que hace
desconfiar de un informe entero.
