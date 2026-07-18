# ADR-0033 — Ecosistema de plugins: registro persistido y runs sellados (M0.6)

**Estado:** Aceptada
**Fecha:** 2026-07-18
**Relacionada con:** RFC-0001 §22 y §45 M0.6; ADR-0029, ADR-0030, ADR-0032

## Contexto

El host WASM de M0.6 ya existía (`df-plugin`: Component Model sin WASI,
capacidades explícitas, fuel/epoch/memoria, registro firmado Ed25519 y seis
componentes de ejemplo adversariales), pero era efímero: nada persistía, y
ningún flujo de proyecto podía ejecutar un plugin sobre evidencia real.

## Decisiones

1. **El registro vive en SQLite y es append-only** (`plugin_registrations`,
   migración 0017): manifiesto firmado, SHA-256 del componente, los bytes
   del componente, clave pública y firma. Qué analizó el corpus es
   auditable para siempre; una versión nueva es una fila nueva, jamás un
   reemplazo.

2. **Todo lo leído del almacén se re-verifica antes de ejecutarse.** Cada
   run reconstruye el paquete firmado desde la base y lo pasa por el host
   completo: firma, hash del componente, coherencia del manifiesto,
   compatibilidad ABI y compilación con type-check. Una base manipulada
   produce un `Conflict` explícito, nunca una ejecución silenciosa.

3. **Runs direccionados por configuración, sellados y con la doctrina
   0013/0016**: digest sobre ABI, esquema de entrada/salida, identidad del
   plugin, hash del componente, límites, política y techo de sujetos.
   Los findings solo se insertan con el run `RUNNING`, nunca se actualizan,
   y el sellado valida contadores contra filas reales.

4. **Los sujetos son contenidos únicos del snapshot analizado**, paginados
   en orden estable y acotados por `max_subjects` (con el sujeto extra
   sondeado para que el techo signifique cola real). El id de sujeto es el
   SHA-256 del contenido: estable entre snapshots e imposible de falsificar
   con sentido.

5. **La política de capacidades es del operador, no del host.** El host
   concede nada por defecto. La fachada concede `SubjectMetadata` (rutas y
   tamaños que el operador ya ve en cualquier informe) y reserva
   `SubjectText` a un opt-in explícito por invocación (`--grant-text`).

6. **Un finding es la afirmación de un plugin, no verdad del host.** Se
   persiste tal cual (código, severidad, mensaje, sugerencias, evidencia,
   sujeto declarado), ligado por el run a la identidad firmada del plugin.
   Un trap, límite agotado o salida malformada cuenta como sujeto fallido —
   evidencia visible, no hueco.

## Alternativas consideradas

- **Registro global (no por proyecto)** — descartado: la base del proyecto
  es la única fuente de verdad y el registro es parte de su cadena de
  evidencia.
- **Conceder capacidades desde el manifiesto del plugin** — descartado: el
  manifiesto *pide*, el operador *concede*; lo contrario invierte la
  autoridad.
- **Normalizar `subject_id` al sujeto suministrado** — descartado: sería
  reescribir la salida del plugin; la fidelidad de la evidencia manda.

## Consecuencias

- La cadena registro→verificación→run→finding queda auditada extremo a
  extremo con eventos `PLUGIN_REGISTERED`, `PLUGIN_RUN_STARTED` y
  `PLUGIN_RUN_COMPLETED` en el ledger.
- Deuda aceptada: `SubjectText` está concedible pero la fachada aún no
  puebla `normalized_text` desde las representaciones M0.4; llegará con el
  cableado de sujetos textuales. La revocación de registros es una decisión
  futura (una fila de revocación append-only, nunca un borrado).

## Tests

`df-facade/tests/plugin_pipeline.rs`: registro firmado real del ejemplo
`metadata-reporter`, ejecución sobre snapshot analizado (2 sujetos → 2
findings INFO), duplicado rechazado, reutilización sellada por digest,
ledger verificado, y componente manipulado tras la firma rechazado sin
almacenar nada. Los límites adversariales del host (loop infinito, bomba de
memoria, salida malformada, intento de filesystem, ABI incompatible) siguen
cubiertos por la suite de `df-plugin`.
