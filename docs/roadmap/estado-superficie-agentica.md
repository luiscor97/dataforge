# Estado de la superficie agéntica

**Última actualización:** 2026-08-01
**Rama:** `feat/agent-drivable-engine`

Documento de coordinación, no de diseño. Existe porque este trabajo se ha
hecho desde varias sesiones en paralelo y una de ellas empezó sin saber que
las otras existían. Si retomas el repo, lee esto primero.

> **Traspaso más reciente:**
> [continuidad-fase-agentica-2026-08-09.md](continuidad-fase-agentica-2026-08-09.md),
> que reconcilia este estado con los hallazgos y fija el orden de PR recomendado.

> Y después [hallazgos-2026-08-01.md](hallazgos-2026-08-01.md): mediciones,
> la procedencia desconocida del score de representante, y el reencuadre de
> M2.3 hacia «el agente propone un perfil» que está pendiente de decisión.

## Dónde está el plan

**El plan es [RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md)**, escrito el
2026-07-21 y **aprobado el 2026-08-01**. Define la escalera de autonomía
L0 → L1 → L2 y su plan de adopción en cinco pasos, con cuatro ADR asociadas
—que siguen en Propuesta, porque aprobar el RFC fija la dirección, no los
detalles:

| ADR | Qué | Estado |
| --- | --- | --- |
| ADR-0041 | `df-rules`: la autoridad determinista del gate | Propuesta |
| ADR-0042 | Consentimiento por política, con presupuesto | Propuesta |
| ADR-0043 | Superficie de herramientas (`df-tools` + `df-mcp`) | Propuesta |
| ADR-0044 | `df-agent`: el bucle de orquestación | Propuesta |

Su tesis, que conviene no perder de vista: **la IA nunca es la autoridad**.
Propone; una regla declarativa verificable decide; lo que no se sabe manejar va
a `revisar/` y el run no se detiene nunca.

## Qué está ya implementado

Todo esto está en `feat/agent-drivable-engine` y pasa la puerta de calidad
completa (440 tests en macOS, 0 fallos; la cifra de Windows es mayor porque
112 puntos del workspace están gateados por plataforma).

| Pieza | Sitio en RFC-0002 |
| --- | --- |
| `Actor::Agent`, distinguible de `cli` en el ledger | **ADR-0043 §3, exactamente como se especificó** |
| `--actor` en la CLI, solo `cli` o `agent` | Adyacente: la superficie diseñada es MCP, no flags |
| `review decide-batch`, atómico, un evento por decisión | No previsto; encaja como capacidad `build` |
| `plan tree`: el árbol de salida antes de aprobar | No previsto; precondición práctica del paso 1 |
| `hash --resume-interrupted` | Robustez de disco lento (RFC-0002 §robustez) |
| `DestinationTaxonomy` leída del perfil (ADR-0040) | Mecanismo del paso 1, subsumido |
| `df-tools`: 25 herramientas en tres clases de capacidad | **ADR-0043 §1**, clase `observe` completa |
| `df-mcp`: servidor MCP por stdio, sin red ni SDK | **ADR-0043 §2** |
| Procedencia de enrutado por operación (migración 0020) | ADR-0040 §3 |
| `df-rules`: cuatro fronteras duras y parámetros con digest | **ADR-0041 §2 y §4** |
| `df-ai::policy`: consentimiento por política con presupuesto | **ADR-0042** |
| `df-agent`: fases, presupuestos y cortacircuitos sin E/S | **ADR-0044** |

`Actor::Agent` se implementó **sin conocer ADR-0043** y coincidió con lo
diseñado. Coincidencia afortunada, no coordinación.

## Conflictos resueltos

**ADR-0040 frente a RFC-0002 — resuelto el 2026-08-01: se subsume.** No
respondían a la misma pregunta. RFC-0002 fija la **política** (qué forma tiene
la salida: `revisar/` como espejo del árbol de salida, cada elemento en su mejor
ubicación estimada, el motivo como metadato); ADR-0040 fija el **mecanismo**
(raíces declaradas y cerradas en vez de constantes en un `match`). RFC-0002
necesitaba ese mecanismo y no lo especificaba: su espejo tiene que ser él mismo
una raíz declarada y sus nombres tienen que quedar reservados frente a los
orígenes.

ADR-0040 pasa a **Aceptada, subsumida en RFC-0002 como mecanismo del paso 1**.
Sus decisiones 1, 2, 3, 4 y 6 se conservan íntegras; la **5 queda reemplazada**
por el espejo, porque trataba `revisar/` como bolsa plana.

El código ya commiteado era **neutro** respecto a esa decisión —`generic`
preserva la salida 1.x byte a byte, con test que lo fija—, así que la resolución
no obliga a tocar nada de lo ya escrito.

**RFC-0002 aprobada el 2026-08-01.** Deja de ser un borrador en una rama sin
fusionar. Sus seis preguntas abiertas quedan atadas cada una al hito que no
puede empezar sin ella; ninguna bloquea M2.1.

## Conflictos abiertos

**La CLI no es la superficie diseñada.** ADR-0043 pone el vocabulario acotado
en la frontera de transporte, con `df-tools` y un servidor MCP, precisamente
para no confiar en el buen comportamiento del modelo. `--actor` en la CLI es
útil pero no sustituye eso: un agente con acceso a la CLI tiene acceso al
shell.

## Qué NO existe todavía

- **Los cuatro crates ya existen** (2026-08-01): `df-tools`, `df-mcp`,
  `df-rules` y `df-agent`. Lo que falta no son crates, es cableado: `df-rules`
  no persiste sus conjuntos ni alimenta el score del planificador, `df-agent` no
  conduce todavía el motor por `df-tools`, y la política de M2.5 no está
  conectada a la ruta de transporte. Cada uno lleva su pendiente escrito en
  [ROADMAP-2.0](ROADMAP-2.0.md).
- Clasificación semántica: el motor sigue enrutando por tipo de operación.
  Es M2.3, y es la precondición de la deduplicación, no una mejora de orden.
- La procedencia extendida del gate autónomo (regla, política, confianza,
  proveedor) que RFC-0002 sella en la transacción del congelado. La de
  enrutado —qué raíz eligió el planificador— sí existe desde la migración
  0020; son cosas distintas.
- El gate en sí: `Capability::requires_authorization` marca las tres
  herramientas `commit`, pero nada lo consulta todavía. La costura está
  declarada para que la clasificación quede fijada antes de que algo dependa
  de ella.
- **Conducción de un trabajo largo (2026-08-10).** Las 25 herramientas son un
  vocabulario completo y **no** una superficie conducible. Deuda reabierta de
  M2.1, y va antes que la clasificación porque sin ella no hay bucle: hay una
  llamada que no vuelve. Detalle en
  [superficie-derivada-del-trabajo-real.md](superficie-derivada-del-trabajo-real.md).
  - Arrancar y esperar siguen **acoplados**: `hash_project` ocupa la única
    sesión stdio durante horas. Faltan `job_start` / `job_status`.
  - Nada registra si un run sigue **vivo**: ni PID, ni host, ni latido en todo
    `crates/`.
  - ~~Los informes grandes no caben~~ **resuelto**: los seis informes con
    detalle devuelven una ventana (`limit`/`offset`, 50 por defecto, techo de
    1.000 que el llamante no puede subir), con los totales sin acotar y
    `pages.<colección>.has_more` siempre explícito. Superficie
    `dataforge.tool-surface/0.3.0`.

## Referencia de corpus

Las cifras que se citan en los ADR de esta línea vienen de un archivo real:
158.219 archivos, 443,9 GB, 28.537 conjuntos de duplicados exactos, 239,7 GB
redundantes. Sobre ese corpus la cola de revisión tiene 5.334 elementos, de
los cuales 3.702 son la misma clase (`EMBEDDED_TREE`) — el dato que justifica
que las decisiones se tomen por clase y no por elemento.

## Si trabajas en paralelo

RFC-0002 y las ADR-0041 a 0044 **ya están en `main`** (PR #36, 2026-08-01), así
que la numeración dejó de estar solo reservada. Siguen vivas
`fix/desktop-usability` y `perf/m101-pipeline-throughput`, además de las de
dependabot: antes de abrir una rama nueva, comprueba `git branch -r`.
