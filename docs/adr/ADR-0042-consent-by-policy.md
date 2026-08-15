# ADR-0042 — Consentimiento por política (divulgación autónoma)

**Estado:** Propuesta
**Fecha:** 2026-07-21
**Relacionada con:** RFC-0002 (consentimiento-por-política); ADR-0034 (IA
asistida BYOK, consentimiento por digest)

> Numeración reservada por RFC-0002; el número final se fija al commitear.

## Contexto

ADR-0034 fija el consentimiento **por digest y por invocación**: para cada
divulgación a la nube, un humano devuelve el SHA-256 exacto del manifiesto de
divulgación. En modo autónomo desatendido eso es imposible: no hay humano al que
preguntar por cada item, y un run puede durar días.

Dos hechos del algoritmo real acotan el problema:

1. **El grueso del trabajo es determinista** (dedup por SHA-256 + auditoría de
   árboles injertados) y **no divulga nada**: no toca ni clave ni red.
2. La IA —y por tanto cualquier divulgación— solo hace falta para la **cola
   ambigua** y para **inventar taxonomía (Modo 2)** cuando no hay árbol canónico.

Es decir, la divulgación autónoma gobierna una **minoría** de los items, no el
flujo entero.

## Decisión

**Una política de divulgación que el humano aprueba una vez, y contra la que se
audita cada invocación.**

1. **La política declara**: qué campos pueden divulgarse (con la redacción por
   defecto de ADR-0034), a qué proveedor, y con qué **presupuesto**
   (nº de invocaciones / tokens / bytes / gasto).
2. **Se congela y se sella su digest** al aprobarla (como el manifiesto de
   ejecución). Cada invocación del agente lleva ese digest en su procedencia.
3. **Auditoría contra política**: una invocación que exceda campos o presupuesto
   se **rechaza antes de tocar clave o red**; queda en la auditoría append-only.
4. **Presupuesto agotado → degradar, no bloquear**: los items ambiguos restantes
   van a `revisar/`. Nunca se detiene el run ni se dispara la factura.
5. **Local-first por defecto**: el modelo local (`--local-exe`, ADR-0034 §4)
   resuelve la cola rutinaria sin divulgación; solo lo verdaderamente ambiguo
   escala a nube **dentro del presupuesto**.
6. **La clave sigue en el almacén del SO** (nunca SQLite, nunca la política);
   sin cambios respecto a ADR-0034.

## Alternativas consideradas

- **Consentimiento por digest en autónomo** — descartado: bloquea o es inviable a
  escala; contradice el requisito de no parar.
- **Sin consentimiento / nube libre** — descartado: riesgo de exfiltración; el
  threat model asume proveedor potencialmente hostil.
- **Solo local, sin nube nunca** — válido como configuración de política (máxima
  privacidad), no como única vía; se pierde capacidad en la cola difícil.

## Consecuencias

- Runs desatendidos con **divulgación acotada y auditable**; las dos pasadas
  deterministas no divulgan nada en absoluto.
- El presupuesto convierte "coste de IA" en un límite duro conocido de antemano.
- **Deuda**: pantalla de escritorio para revisar/aprobar la política (hoy CLI);
  extensión directa del flujo de ADR-0034.
- **Revisar si**: aparece un caso de uso que exija divulgación continua de alto
  volumen (entonces reconsiderar el modelo de presupuesto).
