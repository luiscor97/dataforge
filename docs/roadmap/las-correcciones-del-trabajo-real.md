# Las correcciones del trabajo real

> Qué tuvo que corregir una persona, una por una, hasta que el resultado fue
> aceptable — y qué verbo le faltaba a la máquina cada vez.

## De dónde sale esto

El trabajo original que DataForge existe para reproducir se hizo en julio de
2026 con un agente genérico y un operador humano delante. Quedaron 1.528
mensajes, de los cuales **56 son del operador**. Ese registro es la única
especificación de campo que tenemos: no lo que alguien creyó que hacía falta,
sino lo que hubo que decir en voz alta para que el resultado sirviera.

De esos 56 turnos, descontando saludos y «continúa», quedan **nueve
correcciones**: momentos en que el agente entregó algo y la persona dijo que
no. Son el material de este documento.

Los nombres de expedientes y de personas se han sustituido por descripciones
genéricas. La forma del problema se conserva entera; las identidades no salen
del disco del cliente.

## Las nueve correcciones

### 1. El agente reprodujo el fallo que venía a arreglar

El encargo incluía, explícitamente, arreglar árboles duplicados por
arrastres mal hechos: una carpeta de expedientes que reaparece dentro de uno
de sus propios expedientes. La salida del agente contenía ese mismo anidado,
creado por él, dos niveles.

> «¿por qué has creado [X] con [X] dentro y dentro de ese otro [X]?»

**Lo que faltaba:** que la salida se compruebe contra las mismas anomalías que
se buscaron en el origen. Hoy `analyze` mira el origen; nadie mira el
resultado con los mismos ojos.

### 2. Una regla mecánica clasificó por tamaño lo que se define por propósito

El material audiovisual se apartó por tamaño de archivo. Entre lo apartado
había vídeos cortos de un curso: material de trabajo, no entretenimiento.

> «esos vídeos no los considero películas. son vídeos cortos de un curso.
> ¿serías capaz de recolocarlos en su ruta absoluta?»

**Lo que faltaba:** deshacer una decisión de cubo. Y fíjate en la petición
exacta — *recolocarlos en su ruta absoluta*, es decir, devolverlos a donde
estaban, no a donde el motor crea ahora que van.

### 3. Un subárbol que había que dejar en paz

> «la carpeta [de material escaneado] la puedes dejar como está realmente. no
> es necesario reubicar nada de ahí»

**Lo que faltaba:** una exención de reorganización. DataForge tiene
`hash_exclusions`, que excluye de *hashear*. No tiene «esto se copia tal cual
y no se toca», que es una afirmación distinta y más fuerte.

### 4, 5 y 6. El mismo verbo, tres veces, a tres profundidades

Ésta es la corrección dominante y aparece una y otra vez.

El agente había extraído todos los correos a un árbol paralelo, ordenados por
expediente. Correcto y localizable. Y mal:

> «el asesor va a querer que los correos de [un expediente] estén en su
> correspondiente carpeta de [el árbol de expedientes]»

Más tarde, dentro de una carpeta concreta, exactamente lo mismo un nivel más
abajo:

> «has puesto [expedientes]\CORREOS, y dentro hay carpetas como
> [expedientes]\CORREOS\[una persona] [...] mueve el contenido de las carpetas
> a su correspondiente con [expedientes]\[una persona]»

Y entre medias, la corrección más precisa de todo el registro, que corrige a
la corrección anterior:

> — «colócalo dentro de [el árbol de expedientes]»
> — «**no, me refiero a que ordenes el contenido de esa carpeta en los
> correspondientes**»

Mover la carpeta y repartir su contenido son operaciones distintas, y la
persona tuvo que decirlo dos veces porque la primera se entendió mal.

**Lo que faltaba:** un verbo para *«los hijos de A se corresponden por nombre
con los hijos de B; mete cada uno en el suyo»*. No es copiar, no es
consolidar, no es clasificar. Es **fusionar dos taxonomías paralelas de las
mismas entidades**.

### 7. Un nombre que colisiona, y la decisión sobre cómo llamarlo

> «la quiero mover a [el árbol de expedientes] como carpeta de recursos pero
> puede haber conflicto de nombres con otro tipo de recursos, ¿cómo la
> llamamos?»

**Lo que faltaba:** que una colisión de destino sea una pregunta con
alternativas, no un error. Hoy el ejecutor nunca sobrescribe —correcto— pero
la elección de nombre no es una decisión que el plan sepa representar.

### 8. «Usa el mismo razonamiento, en todas partes»

Después de aceptar la fusión de correos, la persona la generalizó a mano:

> «usa el mismo razonamiento que con los correos [...] ¿lo que contiene la
> primera ruta está ya dentro de la segunda? **¿puedes comprobarlo con todas
> las rutas?**»

**Lo que faltaba:** promover una corrección aceptada a regla. Esto no es una
petición de comodidad: es literalmente el peldaño L2 de RFC-0002 —una regla
declarativa en la puerta— pedido en voz alta por un operador real, sin conocer
el RFC. Es la mejor validación que tiene [ADR-0041] y no estaba escrita en
ningún sitio.

### 9. El cierre

> «mueve los archivos donde toquen y termina de consolidar»

Una frase que resume las ocho anteriores, y que hoy no se puede ejecutar.

## Lo que enseña, junto

**La cola de revisión responde a la pregunta equivocada.**

Hoy un ítem de revisión se resuelve eligiendo una etiqueta de cubo:
`COPY_ACTIVE`, `COPY_REVIEW`, `COPY_SEPARATED`. Es una pregunta de
**clasificación**: *¿qué es esto?*

Ninguna de las nueve correcciones es de clasificación. Las nueve son de
**colocación**: *¿dónde va esto?* Y la respuesta no es una etiqueta, es una
ruta — a veces una ruta que hay que calcular por correspondencia de nombres
entre dos árboles.

Un ítem cuya única respuesta posible es una etiqueta **no puede contener la
respuesta correcta**. Por eso el cubo de revisión de la prueba de campo
contiene 129.379 archivos: no porque el motor no sepa qué son, sino porque la
cola no tiene forma de guardar dónde van.

**Y el resultado no se alcanzó en una pasada.** Se alcanzó en diez días de
bucle: entrega, revisión humana, corrección, nueva entrega. Comparar la
respuesta de una pasada de DataForge con ese resultado y llamar «error» a la
diferencia es medir mal. Lo que falta no es acierto: es el bucle.

## Contraste medido

| | Trabajo humano | DataForge, una pasada |
| --- | --- | --- |
| Entregable colocado | 29.228 archivos | 27.944 |
| Pendiente de revisión humana | **175** | **129.379** |
| Criterio de duplicados | un representante por contenido | ídem, limitado por clasificación |
| Rutas largas conservadas | 347, a propósito | 724 enviadas a revisión |
| «Separado» | 613 archivos, por *contenido* | 142, por *regla mecánica* |

Las dos últimas filas son errores de concepto, no de implementación:

- **Rutas largas.** El humano conservó 347 rutas de 240+ caracteres y lo
  justificó: son verificables con soporte de rutas largas. El motor las copió
  sin un solo fallo y aun así las mandó a revisión. `EXTREME_PATH` no debería
  ser una pregunta.
- **Separado.** Para la persona, «separado» es *esto no es trabajo jurídico*:
  música, películas, vídeo personal. Para el motor es *esto es una caché de
  miniaturas*. Comparten nombre y no comparten concepto.

## Cuánta de la cola es ambigüedad real (medido, 2026-08-15)

Antes de construir M2.7 conviene saber cuánto arregla. La pregunta se puede
contestar hoy, sin escribir motor: para cada anomalía que nombra dos carpetas,
comparar el **conjunto de contenidos** de los dos subárboles. El motor ya tiene
la identidad SHA-256 de los 158.219 archivos; «¿es B una copia de A?» no es una
duda, es una comparación de conjuntos que nadie hace.

Sobre las 4.604 preguntas de carpeta del corpus real:

| Veredicto | Casos | |
| --- | ---: | ---: |
| B contenido en A | 2.155 | 46,8% |
| A contenido en B | 1.547 | 33,6% |
| Parcial, único en ambos | 902 | 19,6% |

**El tipo de anomalía predice el veredicto con exactitud perfecta.** Los 3.702
`EMBEDDED_TREE` son contención estricta, los 3.702, sin una excepción. Los 902
`PARTIAL_TREE_UNIQUE_CONTENT` son parciales, los 902. Ninguna de las 4.604 cae
fuera de lo que su etiqueta ya anunciaba.

Y los «parciales» casi no lo son. Contenidos únicos en el lado más contenido:

| Únicos | Casos | |
| --- | ---: | ---: |
| 1 archivo | 426 | 47,2% |
| 2–5 | 313 | 34,7% |
| 6–20 | 94 | 10,4% |
| 21–100 | 54 | 6,0% |
| más de 100 | 15 | 1,7% |

Casi la mitad se separan de la contención total **por un solo archivo**.
`PARTIAL_TREE_UNIQUE_CONTENT` no es un juicio: es una etiqueta al filo, que un
archivo despistado hace saltar.

De donde sale el tamaño de M2.7:

| Cola de revisión | Ítems |
| --- | ---: |
| Preguntas de carpeta | 4.604 |
| − contención estricta, resoluble midiendo | −3.702 |
| − difieren en ≤5 archivos, resoluble con tolerancia y lista de excepciones | −739 |
| **Ambigüedad real** | **163** |
| − `EXTREME_PATH`, que no debería ser pregunta | −724 |
| Regla de extensión de backup | 6 |
| **Cola resultante** | **169** |

**El trabajo humano terminó con 175 ítems para revisión.** Este cálculo llega a
169 por un camino que no comparte nada con aquél: comparación de conjuntos de
hashes, sin criterio jurídico y sin diez días de correcciones. Dos métodos
independientes coinciden en que **la ambigüedad real de este archivo son unos
170 casos**. Los otros 5.165 ítems de la cola de la 1.0 son trabajo que el
motor ya había hecho y se negaba a usar.

Eso también fija el umbral de M2.7: no «reducir la cola», sino **dejarla en el
orden de 170**, que es donde la dejó una persona.

## Dónde entra en la 2.0

**M2.3 — Clasificación** ya demuestra que sin contexto no hay deduplicación.
Este documento añade su hermana: **sin correspondencia no hay colocación**.
Clasificar dice *qué es*; hace falta además decidir *dónde va*, y son dos
capacidades, no una.

**M2.4 — `df-rules`** recibe su mejor evidencia de campo: la corrección 8 es
un operador pidiendo una regla declarativa aplicada a todo el corpus, sin
saber que eso tenía nombre.

**Propuesto, M2.7 — Colocación por correspondencia.** El verbo que falta:

- Detectar árboles paralelos: dos subárboles cuyos hijos se corresponden por
  nombre y describen las mismas entidades.
- Proponer la fusión como **operaciones de plan**, no como efecto inmediato:
  cada archivo con origen, destino y motivo, revisable y aprobable como
  cualquier otra operación.
- Que una decisión de revisión pueda responder **una ruta**, no solo una
  etiqueta.
- Exención de subárbol: «esto se copia tal cual» (corrección 3).
- Deshacer una decisión de cubo devolviendo a la ruta de origen
  (corrección 2).

Nada de esto relaja una garantía: el origen sigue sin tocarse, no se
sobrescribe nada, todo pasa por plan, manifiesto y verificación. Lo que cambia
es qué puede *expresar* un plan.

## Cabos sueltos de la prueba de campo

Medidos o vistos durante la ejecución del 14–15 de agosto y sin sitio propio
todavía. Se anotan aquí para que no vivan solo en una conversación.

- **El cuello del disco era la fragmentación del origen, no su ancho de banda.**
  Mismo volumen, tres etapas: hash 20,9 MB/s leyendo el archivo original,
  execute 19,7 MB/s leyendo y escribiendo a la vez, **verify 53,4 MB/s**
  releyendo el árbol que el ejecutor acababa de escribir de una sentada. La
  salida quedó contigua; el origen son diez años de fragmentación. El aviso de
  `device_preflight` sobre origen y destino en el mismo disco es correcto en
  dirección y exagerado en magnitud: costó un 6%, no la mitad. Merece una
  matización en el texto, con más medidas antes de tocarlo.
- **Un plan descartado deja sus operaciones `PENDING` para siempre.** El
  ejecutor filtra por `plan_id`, así que es inofensivo, pero cualquier conteo
  global de `plan_operations` las suma. En el corpus real son 205.811 filas
  fantasma tras un solo descarte.
- **La parada temprana por ENOSPC del coordinador paralelo** informa `false` en
  vez de arriesgar una suposición. Correcto y pendiente de cerrar.
- **Ejemplo de contención al 100%**: dos carpetas de imágenes con nombre en dos
  idiomas, 3.797 y 3.624 contenidos, intersección 3.624. Ni un byte del segundo
  falta en el primero. Es el caso que mejor ilustra por qué la pregunta no
  necesitaba a nadie.

## Comprobación pendiente

El contraste con el entregable humano cruza por **(nombre, tamaño)**, no por
SHA-256, y un 4,1% de los archivos entregados no está en este origen. Es señal
fuerte, no prueba. El cruce por hash cuesta unas 1,5 h de disco y está sin
hacer.

La medición de solapamiento sí es exacta: usa las identidades SHA-256 del
propio snapshot, no heurísticas de nombre.

[ADR-0041]: ../adr/ADR-0041-df-rules-canonical-recovery.md
