# ADR-0034 — IA asistida: BYOK, transportes en el borde y consentimiento por digest (M0.7)

**Estado:** Aceptada
**Fecha:** 2026-07-18
**Relacionada con:** RFC-0001 §23 y §45 M0.7; ADR-0029, ADR-0033

## Contexto

El motor de M0.7 (`df-ai`) ya existía: preparación en dos fases con
manifiesto de divulgación, redacción, esquema cerrado de salida, riesgo y
confianza recalculados localmente y un corpus de prompt-injection. Faltaba
lo que lo hace usable: de dónde salen las credenciales, quién habla con la
red y cómo consiente el usuario.

## Decisiones

1. **BYOK en el almacén de credenciales del sistema operativo.** Ni
   Anthropic ni OpenAI ofrecen OAuth a aplicaciones de terceros; el patrón
   es la API key propia del usuario. Se guarda en el Windows Credential
   Manager vía `keyring`, y en ningún otro sitio: nunca SQLite, nunca
   archivos de configuración, nunca el ledger, nunca logs ni mensajes de
   error. La CLI la lee por stdin, jamás por argumentos.

2. **Los transportes viven en el borde (fachada), no en el motor.**
   `df-ai` no enlaza código de red y nunca ve una credencial: implementamos
   su `CloudTransport` en `df-facade` con `ureq` (HTTPS mínimo, rustls).
   El transporte mapea el sobre agnóstico al formato de Anthropic
   (Messages API) u OpenAI (Chat Completions), extrae solo el texto del
   modelo y traduce errores a `ProviderFailure` sin reflejar cuerpos de
   respuesta — un proveedor hostil no puede colar texto por el canal de
   error.

3. **El consentimiento es el digest del manifiesto.** `prepare` sin
   consentimiento es una previsualización pura: muestra campo a campo lo
   que se divulgaría (con las redacciones aplicadas) y no envía nada. Para
   ejecutar hay que devolver el SHA-256 exacto de ese manifiesto; un digest
   distinto se rechaza antes de tocar clave o red. El token de
   consentimiento nunca se persiste: la auditoría prueba *qué* divulgación
   se aceptó, no permite reproducirla.

4. **Ruta local sin nube.** `--local-exe` ejecuta un modelo local absoluto
   bajo `df-process-safety` (Job Object, límites de memoria, tiempo y E/S)
   con el mismo contrato de salida validado. Los ids de evidencia son
   nombres de campo estables, de modo que un proveedor air-gapped es
   reproducible.

5. **Auditoría append-only (migración 0018).** Cada invocación deja una
   fila inmutable con el contrato completo de auditoría del motor más
   columnas indexables, y su evento en el ledger en la misma transacción.

6. **El primer caso de uso es explicar items de revisión.** La evidencia
   enviada son los campos del item (tipo, razón, acción recomendada,
   carpetas), con la redacción por defecto activa. La IA explica y sugiere
   etiquetas; el humano decide en la cola de revisión de siempre.

## Alternativas consideradas

- **OAuth con cuentas de proveedor** — imposible: no existe para terceros;
  además las suscripciones de chat no dan acceso a API.
- **Guardar claves cifradas en la base del proyecto** — descartado: la base
  viaja con el proyecto y el threat model asume que un atacante puede
  tenerla; el almacén del SO es el lugar correcto.
- **reqwest/tokio para los transportes** — descartado: superficie de
  dependencia mucho mayor para dos POST bloqueantes.

## Consecuencias

- Dependencias nuevas ancladas: `keyring` (=3.6.3, windows-native) y
  `ureq` (=3.1.2, rustls), ambas solo en la fachada; `cargo deny` y
  `cargo audit` verdes.
- Deuda aceptada: sin llamada de validación al guardar la clave (se valida
  en el primer uso real), un solo caso de uso (items de revisión) y sin
  pantalla de escritorio todavía; ambos son extensiones directas del mismo
  flujo prepare→consent→execute.

## Tests

`df-facade/tests/ai_pipeline.rs` (Windows): preview completa sin clave,
digest incorrecto rechazado, ejecución aislada con un modelo local
determinista, sugerencia validada con riesgo y confianza recalculados,
auditoría inmutable con ledger válido, y cloud sin clave que falla cerrado
tras el consentimiento. Los transportes tienen tests unitarios de mapeo de
sobre y extracción; el corpus adversarial de prompt-injection sigue en la
suite de `df-ai`.
