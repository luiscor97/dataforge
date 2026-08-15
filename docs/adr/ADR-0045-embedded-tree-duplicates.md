# ADR-0045 — Un duplicado dentro de un árbol probadamente contenido no es contexto desconocido

**Estado:** Parcialmente implementada — insuficiente como está escrita
**Fecha:** 2026-07-29 (implementación y hallazgo: 2026-08-01)
**Relacionada con:** RFC-0001 §15.2, §15.3, §19.4; RFC-0002 (Modo 1, pasada 2);
ADR-0023, ADR-0037; ROADMAP-2.0 M2.3

> Numeración: 0041–0044 están reservadas por RFC-0002.

> ## Hallazgo al implementarla: la decisión 1, sola, no cambia nada
>
> `DuplicateKind::ContainedTreeReplica` y la consulta a `tree_relations` están
> implementadas y son correctas. **Y no tienen ningún efecto observable**, por
> una razón que esta ADR no consideró.
>
> Una relación `TREE_EMBEDDED` **levanta por sí misma un ítem de revisión
> estructural** que cubre todos los archivos de las dos carpetas. Mientras está
> pendiente, todo se enruta a la bolsa de revisión. Y una vez decidido, la
> decisión **sigue siendo una recomendación** — el planificador deja
> deliberadamente que una recomendación anule la consolidación de duplicados,
> con este comentario en el código:
>
> > *Rules and unresolved human review are conservative copy actions. They
> > override duplicate consolidation (never protected-boundary preservation) so
> > an ambiguous occurrence is not silently represented by another path before
> > the user decides.*
>
> Es decir: la clasificación se calcula bien y **nunca se consulta** para estas
> apariciones. Los 108,5 GB que la ADR promete desbloquear siguen sin
> desbloquearse.
>
> **Lo que falta es una decisión sobre esa precedencia**, y no se toma aquí
> porque no es la que esta ADR planteó. Las opciones, a bote pronto:
>
> 1. Que una disposición respaldada por una **prueba** (no por una inferencia)
>    gane a una recomendación ya decidida. Habría que definir qué cuenta como
>    prueba, y el comentario de arriba explica por qué la precedencia actual
>    existe.
> 2. Que una relación `TREE_EMBEDDED` **no genere ítem de revisión** cuando el
>    lado contenido no tiene contenido propio: si el motor ya lo ha probado, no
>    hay nada que preguntar. Es la opción más limpia y la que menos toca el
>    orden de seguridad.
> 3. Dejarlo como está y aceptar que la consolidación exige decidir la clase
>    *y* que la decisión sea de un tipo que no genere recomendación — hoy no
>    existe tal tipo.
>
> Fijado por `crates/df-planner/tests/contained_tree_replicas.rs::a_recommendation_still_overrides_the_proof`,
> para que el hueco quede registrado y no se redescubra.

## Contexto

`classify_duplicate_set` decide el *kind* de un conjunto de duplicados a
partir de una única prueba de contexto: si todas las copias comparten carpeta
y raíz de origen, es `WithinSameContext`; en cualquier otro caso queda
`UnknownContext`. Y `UnknownContext` no lo consolida **ninguna** política,
tampoco `CONSOLIDATE_ALL`, porque §15.2 prohíbe inferir redundancia.

Esa prudencia es correcta. El problema es que el motor **ya tiene la prueba**
en otro sitio y el clasificador no la consulta.

`tree_relations` almacena relaciones `TREE_EMBEDDED`, y su propio `CHECK` fija
la semántica: una relación embebida exige que el lado contenido tenga
`unique_*_files = 0`. Es decir, **la carpeta contenida no tiene nada propio**:
cada contenido suyo existe también fuera. Eso no es una heurística ni una
similitud, es una demostración por hash, del mismo tipo que la que sostiene un
conjunto de duplicados.

### Medición sobre el corpus real

| | |
| --- | --- |
| Redundancia total | 239,7 GB |
| Alcanzable por cualquier política de la 1.0 | 5,45 GB |
| Bloqueada como `UnknownContext` | 234,2 GB |
| **Ya probada redundante por una relación `TREE_EMBEDDED`** | **118,9 GB** |

Esos 118,9 GB son 708 carpetas contenidas *maximales* —excluyendo las
anidadas dentro de otra contenida, para no contar dos veces— con 67.648
archivos. Sin ese filtro la cifra bruta es de 1.222 carpetas y 162,6 GB, que
solapa.

`report tree-relations` publica ahora esa medición, y da **108,5 GB** sobre las
mismas 708 carpetas. La diferencia es real y conviene entenderla: el informe
suma los **contenidos distintos** compartidos, mientras que 118,9 GB es el peso
bruto de los subárboles, que incluye la duplicación interna de cada uno. Como
un plan copiaría cada aparición, el ahorro efectivo se parece más a la cifra
alta; el informe publica la baja a propósito, igual que hace la estimación de
clones, porque prometer un ahorro que luego no aparece es como un destino deja
de caber.

Dicho de otro modo: **la mitad de la redundancia que hoy se declara
inconsolidable ya está demostrada**, y se copia igual porque la prueba vive en
una tabla que el clasificador no mira.

## Decisión

1. **`classify_duplicate_set` consulta las relaciones de árbol.** Una
   aparición que vive dentro de una carpeta que es el lado *contenido* de una
   relación `TREE_EMBEDDED`, cuando el representante del conjunto vive fuera de
   esa carpeta, deja de ser contexto desconocido: es una réplica dentro de un
   subárbol sin contenido propio. Se introduce el kind
   `DuplicateKind::ContainedTreeReplica`.

2. **Solo `TREE_EMBEDDED`, nunca `PARTIAL_TREE_CLONE`.** Un clon parcial tiene
   contenido único en ambos lados —lo garantiza su `CHECK`— y por eso es un
   aviso y no una oportunidad de consolidación (§19.4). Esa frontera no se
   toca: sigue yendo a revisión, y es la deuda que ADR-0023 dejó abierta.

3. **El orden de seguridad de `decide()` no cambia.** Frontera protegida
   primero, representante siempre se materializa, y solo después la política.
   Un `ContainedTreeReplica` dentro de una frontera protegida se preserva
   exactamente igual que hoy.

4. **Sigue siendo opt-in.** `REPORT_ONLY` continúa siendo el valor por defecto
   y no consolida nada. Lo que cambia es que las políticas consolidadoras
   pasan a tener sobre qué actuar; no que actúen sin que nadie lo pida.

5. **La razón lo dice.** La operación registra que se representó por una
   relación de árbol y cuál, no solo que "ya está representado". Una salida
   119 GB más pequeña necesita poder explicar cada byte que no escribió.

## Compatibilidad y contratos

Cambia el contenido de los planes que usen una política consolidadora, así que
mueve contrato: la versión del análisis y la expectativa de
`frozen_contracts` se suben en el mismo commit que implemente esto, según
ADR-0037 §2. Los planes ya aprobados son inmutables y no se recalculan.

`REPORT_ONLY` produce exactamente la misma salida que antes, byte a byte: la
clasificación cambia el *kind*, y con esa política el kind no altera la
disposición.

## Alternativas consideradas

- **Dejarlo en `UnknownContext` y resolverlo por revisión humana.** Es el
  estado actual. Sobre el corpus real son 3.702 elementos de revisión que
  arrastran 129.379 copias, y el resultado es que el 63% del volumen acaba en
  la bolsa de revisión. Pedir criterio humano para algo que el motor ya ha
  demostrado no es prudencia, es no usar la prueba que se tiene.
- **Un `DuplicateKind` para cualquier relación de árbol.** Descartado:
  metería los clones parciales en el mismo saco, y ahí sí hay contenido único
  en ambos lados. La distinción entre "no tiene nada propio" y "los dos tienen
  algo propio" es exactamente la que separa consolidar de revisar.
- **Confiar en la similitud (`similarity`) por encima de un umbral.**
  Descartado: la similitud es Jaccard sobre contenidos, y un 0,99 sigue
  significando que algo único puede quedarse fuera. `unique_files = 0` es una
  afirmación categórica; un umbral no lo es.
- **Que lo decida el agente.** Descartado para este caso: cuando existe una
  prueba determinista, gastarla en una llamada a un modelo es peor en coste y
  en auditoría. El agente es para lo ambiguo; esto no lo es.

## Consecuencias

**Positivas.** Desbloquea ~118,9 GB de los 234,2 GB que hoy no puede tocar
ninguna política, usando evidencia ya calculada y sin releer un byte. Reduce
la cola de revisión en su clase más grande. Y hace que la deduplicación
dependa de una demostración en lugar de una coincidencia de carpeta.

**Negativas.** Amplía la superficie de lo consolidable, que es la superficie
donde una salida puede quedar peor organizada. Mitigado porque nada se borra,
el origen queda intacto y toda la operación es reversible borrando el árbol de
salida. Y obliga a subir contrato.

**Neutras.** No toca `REPORT_ONLY` ni `PRESERVE_ALL`.

**Revisar si.** Si al medir sobre otro corpus resulta que las relaciones
`TREE_EMBEDDED` aparecen mayoritariamente dentro de fronteras protegidas, el
desbloqueo real sería mucho menor que el medido aquí y habría que reconsiderar
si el cambio de contrato se justifica.
