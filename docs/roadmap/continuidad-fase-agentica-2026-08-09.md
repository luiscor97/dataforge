# Continuidad de la fase agéntica — 2026-08-09

**Estado:** traspaso operativo. No sustituye RFC-0002 ni las ADR; fija el orden
de trabajo recomendado después de contrastar el roadmap con el estado real de
`feat/agent-drivable-engine`.

**Rama revisada:** `feat/agent-drivable-engine`

**HEAD al redactar:** `2c4f7d1130a36f38ff013d2dd1fcfa3edef599ca`

**PR:** [#45](https://github.com/luiscor97/dataforge/pull/45)

## Orden de lectura al retomar

1. [Estado de la superficie agéntica](estado-superficie-agentica.md).
2. [Hallazgos del 2026-08-01](hallazgos-2026-08-01.md).
3. [RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md).
4. [Roadmap 2.0](ROADMAP-2.0.md).
5. [Estructura de M2.4 a M2.6](estructura-m2.4-m2.6.md).
6. Este documento, como secuencia de ejecución y punto de reanudación.

## Tesis que no cambia

La IA nunca es la autoridad. Propone una clasificación o un perfil; una regla
determinista decide sobre evidencia local; lo que no pueda defenderse va a
`revisar/`; el run no se detiene durante la fase larga.

La autonomía cambia quién prepara y autoriza el plan. No cambia las garantías:

- origen inmutable;
- sin borrado ni sobrescritura de archivos de usuario;
- SQLite como fuente de verdad;
- estado y ledger en la misma transacción;
- clientes sobre la fachada;
- verificación independiente.

## Estado real

La fase no empieza desde cero:

- M2.1 tiene `df-tools`, `df-mcp`, atribución `Actor::Agent`, decisiones por
  lote y vista del plan;
- M2.2 tiene raíces declaradas por perfil y procedencia de enrutado;
- M2.3 tiene la primera clasificación basada en `TREE_EMBEDDED`, pero una
  recomendación estructural todavía anula su efecto;
- M2.4 tiene el núcleo puro de `df-rules`, cuatro fronteras duras y parámetros
  con digest, pero no persistencia ni gate efectivo;
- M2.5 tiene la decisión pura de consentimiento por política, pero no
  persistencia ni conexión al transporte;
- M2.6 tiene fases, presupuestos, cortacircuitos y la garantía de no bloqueo,
  pero todavía no conduce el motor.

No se deben volver a crear esos crates ni declarar hitos terminados porque sus
tipos existan. Lo pendiente es decisión, cableado, persistencia y prueba real.

## Decisiones que deben preceder al siguiente bloque grande

### 1. Reencuadrar M2.3 alrededor de perfiles

La evidencia medida aconseja que el modelo no juzgue 158.219 archivos uno a
uno. Debe proponer una vez un perfil pequeño, auditable y reutilizable; el motor
aplica después ese perfil de forma determinista.

Esto requiere una ADR antes de implementar más clasificación semántica. Debe
definir:

- esquema y límites del perfil propuesto;
- evidencia que el modelo puede consultar;
- validación local y rechazo *fail-closed*;
- revisión y aprobación humana del perfil;
- procedencia, versión y digest;
- aplicación reproducible sobre todo el corpus;
- comportamiento cuando el perfil no puede ubicar un elemento.

Hasta cerrar esta decisión no conviene persistir parámetros cuya semántica
pueda cambiar con el nuevo perfil.

### 2. Resolver ADR-0045

La recomendación es la opción 2 documentada en ADR-0045:

> Una relación `TREE_EMBEDDED` cuyo lado contenido tiene cero archivos únicos
> no genera una revisión estructural, porque el motor ya ha demostrado la
> redundancia por hash.

Es una recomendación pendiente de aprobación del autor, no una decisión tomada
por este documento. Es la opción preferida porque elimina una pregunta
redundante sin cambiar la precedencia global de seguridad.

La implementación, si se aprueba, debe mantener:

- `PARTIAL_TREE_CLONE` siempre a revisión;
- fronteras protegidas siempre a revisión;
- `REPORT_ONLY` y `PRESERVE_ALL` byte a byte compatibles;
- procedencia de la relación que justificó no materializar una copia.

### 3. Auditar una muestra de criterio humano

Los pesos actuales del representante proceden probablemente de la ejecución
original conducida por un modelo y no están validados. No se debe ajustar el
motor para reproducir a ciegas aquella salida.

El autor debe revisar una muestra estratificada por clase. El resultado será la
referencia para:

- validar o cambiar los pesos;
- decidir fronteras del perfil jurídico;
- calibrar confianza;
- medir el modo sombra posterior.

## Roadmap por bloques de PR

### PR 0 — Cerrar la fundación actual

Objetivo: cerrar PR #45 sin seguir acumulando funcionalidad.

- actualizar su resumen al alcance y cifras reales;
- confirmar contratos y migraciones;
- ejecutar la puerta de calidad en el HEAD final;
- registrar las decisiones anteriores como bloqueos explícitos;
- **cerrar la clase `commit` en `df-mcp` mientras no exista el gate**: la
  superficie llega a `main` en este PR y la autoridad determinista no llega
  hasta el PR 5. Sin esto, durante cinco bloques cualquiera que compile `main`
  y apunte un modelo al servidor tiene `approve_plan` y `execute_plan` sin
  humano y sin regla — que no es L1 ni L2, sino L2 sin `df-rules`, la única
  combinación que RFC-0002 descarta;
- fusionar antes de abrir el siguiente bloque de implementación.

### PR 1 — Hacer conversacional la superficie

Objetivo: que una sesión corta pueda observar y controlar trabajos grandes.

- acotar y paginar `duplicate_report` y `structural_review_queue`;
- devolver agregados por defecto;
- separar arrancar un trabajo de consultar su progreso;
- impedir que `df-mcp` quede bloqueado durante el reloj de bytes;
- conservar SQLite como único canal entre conversación y trabajo.

No introducir todavía un daemon permanente.

### PR 2 — Vitalidad y worker desacoplado

Objetivo: distinguir un trabajo vivo de uno interrumpido antes de desacoplarlo.

- registro de PID, host, fase y latido;
- detección conservadora de propietario vivo;
- toma de control explícita y auditada de un trabajo muerto;
- worker dueño de hashing, copia y verificación;
- MCP limitado a ordenar y sondear.

### PR 3 — ADR y motor de perfil rico

Objetivo: que el agente proponga política reutilizable en vez de decidir cada
archivo.

- aprobar la ADR del reencuadre;
- validar un perfil propuesto antes de guardarlo;
- versionarlo y sellar su digest;
- aplicarlo de forma determinista;
- enrutar lo no ubicable a `revisar/_sin-ubicar/` con motivo;
- conectar evidencia documental y multimedia solo mediante reglas declaradas.

### PR 4 — Cerrar ADR-0045 y la clasificación determinista

Objetivo: usar toda prueba local antes de pedir criterio humano o IA.

- aplicar la decisión aprobada de ADR-0045;
- completar los estados de árbol injertado;
- elegir representantes de forma reproducible;
- conservar las fronteras protegidas;
- medir el efecto sobre la cola y el volumen del corpus real.

### PR 5 — Completar M2.4 y entregar L0

Objetivo: convertir `df-rules` en la autoridad efectiva.

- migración 0021 y conjuntos de reglas append-only;
- digest verificado en cada uso;
- parámetros validados contra la muestra auditada;
- `Authorize | Review | Deny` con regla y evidencia;
- ninguna herramienta `commit` ejecutable sin pasar por el gate;
- procedencia sellada en la operación.

Al cerrar este bloque existe **L1 — Copiloto** en el sentido de RFC-0002: el
agente observa y propone; el humano ocupa el gate y autoriza el lote. (L0 —la
IA solo explica— está entregado desde M0.7 y no es el resultado de ningún PR
de esta lista.)

### PR 6 — Completar M2.5

Objetivo: permitir asistencia externa bajo consentimiento y presupuesto.

- resolver primero si el contrato incluye tokens, porque la documentación los
  menciona y el tipo actual solo contiene llamadas, bytes y gasto;
- migración 0022;
- política, consumo y rechazos auditados antes de clave o red;
- proveedor, modelo y campos exactos;
- agotamiento de presupuesto como degradación a revisión;
- prueba con transporte interceptado de que una llamada rechazada no sale.

### PR 7 — Completar M2.6 y entregar L1

Objetivo: ejecutar el bucle completo exclusivamente mediante `df-tools`.

- intención → inventario → clasificación → plan → reglas → congelación →
  ejecución → verificación → informe;
- `dry-run` hasta congelación incluida;
- reanudación desde manifiesto y ledger;
- pre-vuelo de espacio;
- presupuestos y cortacircuitos;
- `revisar/_ilegible/` y `revisar/_verificacion-fallida/` con evidencia, sin
  crear archivos vacíos que aparenten ser el original;
- mapa completo origen → destino;
- procedencia de regla, política, confianza y proveedor.

Al cerrar este bloque **L1 queda completo**: el humano aprueba el lote una vez y
la fase larga termina sin volver a preguntar. Sigue siendo L1 porque quien
autoriza es el humano; lo que cambia es que solo se le pregunta una vez.

### PR 8 — Modo sombra

Objetivo: calibrar sin aplicar decisiones autónomas.

- comparar propuestas del agente con decisiones humanas;
- medir por clase, perfil y dominio;
- detectar falsas autoaprobaciones;
- comprobar estabilidad entre ejecuciones;
- probar varios corpus y no solo el jurídico.

Los umbrales de confianza deben salir de estos datos, no fijarse por intuición.

### PR 9 — L2 autónomo acotado y 2.0

L2 empieza como opt-in y exige:

- destino nuevo y vacío;
- perfil, reglas y política sellados;
- topes de tiempo, operaciones, espacio, ambigüedad y gasto;
- fallback universal a revisión;
- origen verificado como inmutable;
- los nueve criterios de cierre de ROADMAP-2.0 sobre el corpus real.

## Carriles que no se mezclan

- `fix/desktop-usability` sigue siendo producto e instalador.
- `perf/m101-pipeline-throughput` sigue siendo rendimiento; no debe cambiar el
  executor autónomo antes de estabilizar recuperación y procedencia.
- La UI final se diseña sobre un L1 funcional, no como sustituto del motor.
- No se añade más IA antes de agotar evidencia y reglas deterministas.

## Encargo para retomar desde otro equipo

```text
Trabaja sobre luiscor97/dataforge. Empieza comprobando si la PR #45 sigue
abierta o ya fue fusionada y no asumas que el HEAD de este documento sigue
vigente.

Lee, en este orden:
1. docs/roadmap/estado-superficie-agentica.md
2. docs/roadmap/hallazgos-2026-08-01.md
3. docs/roadmap/continuidad-fase-agentica-2026-08-09.md
4. docs/rfcs/RFC-0002-autonomy-ladder.md
5. docs/adr/ADR-0045-embedded-tree-duplicates.md

No recrees df-tools, df-mcp, df-rules ni df-agent: ya existen. No implementes
migraciones 0021–0023 hasta resolver la ADR del perfil y el contrato de
presupuesto. No conviertas la recomendación de ADR-0045 en decisión sin
aprobación del autor.

La próxima intervención debe confirmar el estado de CI y elegir un único bloque
del roadmap. Mantén fuera las ramas de desktop y rendimiento. Para cada cambio:
commits pequeños firmados con DCO, contratos y ADR en el mismo commit cuando
corresponda, y puerta de calidad completa antes de declarar el bloque cerrado.
```

## Punto de llegada

El siguiente producto no es L2. Es **L1 supervisado, completo, reanudable y no
bloqueante**. L2 es una promoción posterior basada en modo sombra y evidencia.
