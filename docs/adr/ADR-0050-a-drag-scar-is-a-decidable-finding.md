# ADR-0050 — Una cicatriz de arrastre es un hallazgo decidible

**Estado:** Propuesta
**Fecha:** 2026-08-18
**Relacionada con:** RFC-0001 §15.2 (no inferir redundancia), §25 (reglas
declarativas); RFC-0002 (la IA propone, una regla verifica); ADR-0037
(contratos congelados); ADR-0045 (réplica de árbol contenida)

## Contexto

El detector de cicatrices de arrastre —una carpeta que repite el nombre de un
ancestro y no contiene nada propio— existe desde M2.x, y desde entonces el
invariante del planner se niega a colocar una en el árbol activo.

Lo que nunca existió fue una forma de responderle.

En el archivo real eso dejó un plan **imposible de aprobar** por dos ramas, y
una de ellas era `ESCANER\DOCUMENTOS ESCANER\ESCANER`. Sobre esa carpeta el
dueño del archivo había dicho, con estas palabras:

> *«la carpeta escaner la puedes dejar como esta realmente. no es necesario
> reubicar nada de ahi»*

El invariante tenía razón: la rama no contiene nada propio. La instrucción
también. Y no había ningún sitio donde escribirla, porque la cicatriz nunca
llegaba a la cola de revisión: se calculaba al pedir el informe, no durante el
análisis, así que no era una anomalía y no generaba item.

**Una frontera dura sin forma de registrar el consentimiento no es una
frontera: es un callejón sin salida.**

## Decisión

`AnomalyKind::DragScar`. La cicatriz se emite como anomalía durante el
análisis, con `requires_review = true`, enlazada a su propia carpeta y con la
medición en la evidencia: ocurrencias, contenidos distintos, y el cero que
importa —contenidos únicos de esa rama—.

Tres consecuencias, y la primera es la que más cambia:

1. **Pendiente, sus contenidos se quedan en revisión.** Ya no se escriben en el
   árbol activo para avisar después. Negarse a colocar es mejor que informar de
   que se colocó.

2. **Decidida, el invariante calla sobre ella.** Solo una decisión la silencia;
   una pendiente sigue levantando el problema, que es justo para lo que se
   levanta. Repetir un aviso que alguien ya contestó es cómo se enseña a
   ignorarlo.

3. **La recomendación sigue siendo `COPY_REVIEW`.** El motor no decide esto por
   su cuenta; deja de negarse a que se lo digan.

## Alternativas consideradas

**Relajar el invariante.** Quitarlo o bajarlo a aviso resuelve el bloqueo y
pierde la garantía. El invariante nació de un caso real: 11.816 archivos sin
nada propio copiados al árbol entregado porque nadie verificó una decisión
razonable.

**Una excepción declarativa en el perfil.** Una lista de rutas exentas sería
específica del corpus, no del dominio, y el perfil se compila. Además una
exención no lleva razón escrita ni actor ni fecha; una decisión sí.

**Dejarlo como está y documentar el rodeo.** El rodeo es cambiar la política de
duplicados, que cambia mucho más que las dos carpetas en cuestión.

## Consecuencias

- **Migración 0026**, que reconstruye `structural_anomalies` para ampliar su
  `CHECK`. SQLite no altera restricciones, así que la tabla se recrea y **se
  recrean sus cuatro triggers**: solo-añadir, propiedad de snapshot y sellado
  tras completar. Un `DROP TABLE` se lleva los triggers por delante, y esas
  garantías no son de las que pueden caducar en silencio porque cambió una
  lista de valores.
- **ADR-0037**: la cadena de migraciones pasa de 25 a 26. Es el bump
  deliberado que esa política contempla, y este documento es el ADR que lo
  acompaña.
- El invariante deja de disparar sobre cicatrices pendientes, porque ya no hay
  ninguna colocada activamente. Sigue siendo el respaldo para el caso que
  motivó su existencia: contenidos colocados sin nada en el registro que lo
  pidiera.
