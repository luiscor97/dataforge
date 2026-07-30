# Estado de la superficie agéntica

**Última actualización:** 2026-07-29
**Rama:** `feat/agent-drivable-engine`

Documento de coordinación, no de diseño. Existe porque este trabajo se ha
hecho desde varias sesiones en paralelo y una de ellas empezó sin saber que
las otras existían. Si retomas el repo, lee esto primero.

## Dónde está el plan

**El plan es [RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md)**, borrador del
2026-07-21, en la rama `design/rfc-0002-autonomy` (todavía sin fusionar).
Define la escalera de autonomía L0 → L1 → L2 y su plan de adopción en cinco
pasos, con cuatro ADR asociadas:

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
completa (450 tests, 0 fallos).

| Pieza | Sitio en RFC-0002 |
| --- | --- |
| `Actor::Agent`, distinguible de `cli` en el ledger | **ADR-0043 §3, exactamente como se especificó** |
| `--actor` en la CLI, solo `cli` o `agent` | Adyacente: la superficie diseñada es MCP, no flags |
| `review decide-batch`, atómico, un evento por decisión | No previsto; encaja como capacidad `build` |
| `plan tree`: el árbol de salida antes de aprobar | No previsto; precondición práctica del paso 1 |
| `hash --resume-interrupted` | Robustez de disco lento (RFC-0002 §robustez) |
| `DestinationTaxonomy` (ADR-0040) | Ver conflicto abajo |

`Actor::Agent` se implementó **sin conocer ADR-0043** y coincidió con lo
diseñado. Coincidencia afortunada, no coordinación.

## Conflictos abiertos

**ADR-0040 frente a RFC-0002.** Las dos responden a "dónde aterriza lo
dudoso". RFC-0002 lo hace mejor: `revisar/` espejo del árbol de salida, con
cada elemento en su mejor ubicación estimada, y el motivo como metadato. Lo
que ADR-0040 aporta y RFC-0002 no cubre es el mecanismo: raíces declaradas y
cerradas en vez de constantes en un `match`. Hay que decidir si se subsume.

El código ya commiteado es **neutro** respecto a esa decisión: `generic`
preserva la salida 1.x byte a byte, con test que lo fija.

**La CLI no es la superficie diseñada.** ADR-0043 pone el vocabulario acotado
en la frontera de transporte, con `df-tools` y un servidor MCP, precisamente
para no confiar en el buen comportamiento del modelo. `--actor` en la CLI es
útil pero no sustituye eso: un agente con acceso a la CLI tiene acceso al
shell.

## Qué NO existe todavía

- `df-rules`, `df-tools`, `df-mcp`, `df-agent`: ninguno de los cuatro crates.
- Clasificación semántica: el motor sigue enrutando por tipo de operación.
- La procedencia extendida del gate autónomo (regla, política, confianza,
  proveedor) que RFC-0002 sella en la transacción del congelado.

## Referencia de corpus

Las cifras que se citan en los ADR de esta línea vienen de un archivo real:
158.219 archivos, 443,9 GB, 28.537 conjuntos de duplicados exactos, 239,7 GB
redundantes. Sobre ese corpus la cola de revisión tiene 5.334 elementos, de
los cuales 3.702 son la misma clase (`EMBEDDED_TREE`) — el dato que justifica
que las decisiones se tomen por clase y no por elemento.

## Si trabajas en paralelo

Hay varias ramas vivas (`design/rfc-0002-autonomy`, `fix/desktop-usability`,
además de las de dependabot). Antes de abrir una nueva, comprueba
`git branch -r` y los ADR propuestos: la numeración 0041–0044 está **reservada
por RFC-0002** aunque su rama no esté fusionada.
