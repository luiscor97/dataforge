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

## Qué falta para que esto sea evidencia completa

- **El informe final al asesor** (`INFORME_FINAL_ORDENACION_...docx` y su PDF)
  vive en `D:\DataForge_Salida\05_Asesoria_Juridica_v2\00_INFORME_DE_ORGANIZACION\`.
  No está en el repositorio y contiene datos del cliente, así que no debe
  entrar tal cual; sí valdría un extracto con los criterios y sin nombres.
- **El manifiesto SHA-256 de cierre** (29.239 entradas) es lo que permitiría
  comparar una salida de DataForge contra esta, archivo a archivo. Es la
  prueba más valiosa que existe y **sigue fuera del repositorio**.
- La diferencia de 2.313 archivos entre lo citado y lo medido.

Sin nombres de cliente: este documento describe formas y recuentos a
propósito. Las rutas y nombres reales viven en el disco, no aquí.
