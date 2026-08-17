# El criterio, leído de las dos partes

**Fuente:** la transcripción completa de la sesión original (57 turnos,
2026-06-29 → 2026-07-10, 339.430 caracteres) más los artefactos de auditoría
que sobrevivieron en `DataForge_Audit_Work`.

Hasta ahora este repositorio citaba el trabajo original por sus cifras finales.
Las cifras no son el criterio. El criterio son las decisiones que se tomaron
para llegar a ellas, y **la mitad de ellas están en las respuestas, no en las
preguntas** — lo que se decidió, lo que se descartó, y sobre todo lo que se
revirtió después de ver el resultado.

El criterio de aceptación no fue técnico. Fue, palabra por palabra:

> *«que el asesor no tenga desconfianza porque se haya podido perder material
> alguno»*

Todo lo demás se deriva de eso.

---

## El orden en que se hizo

No es el orden que uno diseñaría en frío, y esa es exactamente la razón de
registrarlo. Cada paso está donde está porque el anterior enseñó algo.

**1. Inventario primero, solo metadatos.** 156.962 entradas, 0 errores de
lectura. Nada de contenido todavía.

**2. Separar media ANTES de hashear.** Música y vídeo salen del universo
documental *antes* de calcular una sola huella: 2.281 archivos, 136,7 GB,
copiados aparte conservando su ruta original. Dos razones explícitas, y las dos
importan: *«evita que una película de varios GB mande sobre la lógica de
expedientes»*, y quita 136 GB del hash. El documental quedó en 154.681 archivos
y 339,5 GB.

**3. Hash del resto.** SHA-256 completo, sin muestreo. 154.681 archivos,
0 errores, y la decisión de no parar a mitad: *«mantengo la pasada hasta el
final para que las decisiones no dependan de tamaños o nombres, sino de hashes
reales»*.

**4. Representante por grupo de clones exactos**, elegido por profundidad,
nombre, ruta y antigüedad. 47.982 grupos, 27.692 de ellos con duplicados.

**5. El destino conserva la estructura relativa del representante**, no una
bolsa plana: *«para no convertir el archivo limpio en una bolsa plana de
documentos sin contexto»*.

**6. Auditoría de huérfanos** del destino contra el manifiesto. 0.

**7. Segunda pasada estructural sobre la base ya hasheada**, sin volver a leer
el disco. Aquí aparecen los 131 grupos de árbol injertado.

**8. Clasificación jurídica con una razón explícita por archivo**:
`asesoria_main`, `correos`, `periciales`, `soporte_juridico`,
`revision_origen_mixto`, `excluido_no_juridico`. El 40 % de las decisiones fue
excluir.

**9. Copia v2 con verificación SHA-256 en destino, archivo a archivo.** Más
lenta que una copia simple, y a propósito.

**10. Corrección iterativa**, que es donde vive la mayor parte del criterio real
— y la sección siguiente.

**11. Informe final para el asesor**, con manifiesto y trazabilidad dentro de la
propia carpeta entregada.

## Los invariantes que nunca se movieron

- **El origen no se toca.** Repetido en cada turno, sin excepción.
- **Nunca borrar.** Lo que sobra se aparta a un cubo con nombre —
  `97_Duplicados_Exactos`, `98_Temporales_Excluidos`, `99_Excluido_No_Juridico`
  — y se puede recorrer.
- **Al reorganizar se mueve, no se copia.** Textual: *«Si copiara los `.eml` a
  `01` y dejara los mismos en `02`, volveríamos a tener duplicados exactos
  activos»*.
- **Colisión de nombre con hash distinto → sufijo, jamás sobrescribir.**
- **Hash antes y después de cada movimiento.**
- **Nunca deduplicar por nombre.** En periciales hay 422 hashes distintos
  compartiendo nombre de archivo.
- **Cada asunto pericial es una unidad cerrada.** Una foto que aparece en dos
  asuntos se señala, no se fusiona.
- **Lo que carece de contexto se queda en revisión.** No se fuerza la
  clasificación: *«si una carpeta tiene contexto débil o puede depender del
  criterio del despacho, se queda en 90»*.
- **Sin política expresa no se aplican reglas de conservación.** *«prefiero no
  excluir por antigüedad salvo regla expresa»*.

## Lo que se revirtió, que es lo más útil

Un criterio que nunca cambió de opinión no se probó contra nada.

**Media dejó de ser una categoría final y pasó a ser una cuarentena.** El
disparador fue un caso concreto: *«esos vídeos no los considero películas, son
vídeos cortos de un curso»*. De ahí salieron cuatro tandas de reintegración —
129, luego 155, luego 58, luego 6 representantes — de cursos LOPD/RGPD,
mediación, peritos y periciales, cada una deduplicada por huella contra lo ya
integrado. La regla que quedó: **el tamaño no clasifica; el contexto de la ruta
sí**, y la duda se queda fuera con informe.

**«No colapsar nombres repetidos» se convirtió en «colapsar los humanos, no los
técnicos».** Primero: *«hay muchas repeticiones que son legítimas... la regla
segura es colapsar solo raíces principales repetidas al principio»*. Dos días
después se normalizaron 183 rutas `X\X` — `VENEZUELA\VENEZUELA`,
`KONUS ESPAÑA S.L\KONUS ESPAÑA S.L`, `PROCESAL I\PROCESAL I` — saltando
componentes técnicos o numéricos, con hash y sufijo en colisión. La frontera
final no es «raíz conocida» sino **«nombre humano repetido adyacente sí,
componente técnico o numérico no»**.

**`02_Correos` dejó de ser «todos los correos».** Los que tienen asunto
equivalente inequívoco viven dentro de su expediente, en `01\<ASUNTO>\CORREOS`:
1.161 archivos movidos. Los volcados globales (`CORREOS FIN INCREDIMAIL`) se
quedaron donde estaban, con razón explícita: *«si los meto en un expediente sin
saber, ensucio más que arreglo»*.

**`04_Soporte_Juridico` se renombró.** No por técnica sino por audiencia:
`05_RECURSOS_JURIDICOS_Y_FORMATIVOS`, *«porque para el asesor suena menos
técnico y más natural»*. El destinatario del árbol forma parte del criterio.

**`ESCANER` se dejó intacto por orden expresa** y se respetó en todas las tandas
posteriores, incluida la que vació `90` entera.

**Y una detección que resultó ser un falso positivo:** `JAVIER\IMPUESTOS
TRIMESTRALES\JAVIER` parecía arrastre y era una carpeta de contribuyentes por
persona. Se revisó y se documentó como legítima, sin tocarla.

---

## Qué de esto tiene DataForge hoy

| Paso del criterio | Estado |
| --- | --- |
| Inventario y hash con evidencia | **implementado** |
| Representante por grupo de clones | **implementado** (`df-db/dedup.rs`) |
| Destino conserva estructura relativa | **implementado** |
| Origen inmutable, nunca borrar | **implementado**, es invariante del motor |
| Sufijo en colisión | **implementado** (`COPY_WITH_SUFFIX`) |
| Nunca deduplicar por nombre | **implementado** (§15.2, informe de colisiones) |
| Contextos protegidos por perfil | **implementado**, y por fin seleccionable con aviso |
| Árboles injertados y raíces injertadas | **implementado** (`report grafted-trees`, `drag-scars`, `grafted-roots`) |
| Cubos de apartado con nombre | **implementado** (taxonomía ADR-0040) |
| Paquete de entrega con manifiesto | **implementado** (`deliver`) |
| **Media como cuarentena previa al hash** | **ausente**: hay `df-media` y exclusiones de hash, pero declaradas por el operador, no como fase que retire el material pesado del universo documental antes de decidir |
| **Componente adyacente repetido `X\X`** | **ausente**: el detector nuevo encuentra raíces de primer nivel injertadas, no `KONUS\KONUS` |
| **Clasificación por categorías con razón por archivo** | **parcial**: existe el enrutado a revisión, no la taxonomía jurídica completa |
| **Correos dentro de su expediente** | **ausente** (M2.7) |
| **Mover en vez de copiar al reorganizar** | **fuera de modelo**: DataForge solo copia a un destino nuevo y nunca mueve, así que la reorganización iterativa que hizo el trabajo original no tiene equivalente |

Las tres últimas filas son el hueco real, y no son un fallo: son la diferencia
entre **construir una salida de una pasada** —lo que DataForge hace— y
**converger a una salida en once días de correcciones**, que es lo que produjo
el resultado que se quiere reproducir.

## La consecuencia incómoda

El trabajo original no acertó a la primera. Acertó **iterando sobre su propia
salida**, con nueve correcciones que solo fueron visibles al ver una entrega
entera: los correos, el nombre de la carpeta de recursos, la triple anidación de
`AGENTES COMERCIALES`, los vídeos del curso que parecían ocio.

Un motor que produce una salida verificada de una sola pasada no llega ahí por
ser mejor en la pasada. Llega, si llega, porque **cada una de esas nueve
correcciones esté codificada como regla antes de empezar** — que es exactamente
lo que este documento existe para permitir, y por lo que las cifras finales no
bastaban.
