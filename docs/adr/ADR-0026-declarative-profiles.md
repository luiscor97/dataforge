# ADR-0026 — Perfiles declarativos y fronteras protegidas

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 §18.3, §25.2, §25.4, §15.2, regla 9; ADR-0024, ADR-0025

## Contexto

La clasificación de contextos (ADR-0024) llegó con los marcadores **escritos a
mano en `df-db/src/context.rs`** y el parámetro `profile` ignorado
(`let _ = profile;`). Consecuencia: `ContextKind::Protected` y
`is_protected_boundary` existían en el dominio, y la regla 9 estaba
implementada y probada en la capa de políticas (ADR-0025), pero **nada podía
marcarse nunca como protegido**. La protección era código correcto sin nadie a
quien proteger.

Además, M0.2 exige un **perfil jurídico sintético** como criterio de
aceptación.

## Decisiones

1. **Los perfiles son datos, no código.** Viven en
   `profiles/<id>/profile.json` y declaran `generic_markers` (nombre +
   penalización §18.3) y `protected_markers` (nombre + razón). Qué carpeta es
   una frontera pasa a ser algo revisable en un PR por alguien del dominio, no
   una constante enterrada en un `.rs`.

2. **Embebidos en compilación con `include_str!`**, igual que las migraciones
   SQL. Se evita resolver rutas en runtime y, sobre todo, se evita que un
   archivo suelto junto al ejecutable cambie en silencio qué se consolida. Los
   perfiles aportados por el usuario son cuestión de plugins (M0.6).

3. **JSON, no YAML.** El §25.1 del RFC ilustra las *reglas* con YAML, pero el
   §5.7 (formatos abiertos) lista JSON y no YAML, el workspace ya parsea JSON
   con `serde_json`, y las crates de YAML mantenidas no compensan una
   dependencia nueva (`serde_yaml` está descontinuada y chocaría con
   `cargo deny`). Se revisará cuando llegue el rule engine (§25.1).

4. **Herencia de un solo nivel** (`inherits`). `legal` reutiliza los
   contenedores genéricos de `generic` y añade sus fronteras. Más de un nivel
   se rechaza: la resolución debe ser trivial de auditar.

5. **Un perfil desconocido cae a `generic`**, que no protege nada pero tampoco
   consolida nada por sí solo (§25.4). El fallback nunca puede conceder
   *menos* protección de la esperada por accidente.

6. **Coincidencia `exact` por defecto, `prefix` cuando se declara.** Las
   carpetas reales se llaman `Expediente 1234-2020`, así que con solo igualdad
   exacta el perfil jurídico no protegería casi nada. `prefix` exige que tras
   el marcador venga un separador o un dígito, de modo que `expediente` casa
   `expediente 1234`, `expediente_12` y `expediente2020`, pero **no**
   `expedientes` ni `exposicion`. El default sigue siendo `exact` porque una
   coincidencia laxa es un problema de seguridad en ambos sentidos: penaliza
   ubicaciones legítimas o protege tanto que la consolidación deja de
   funcionar en silencio.

7. **`Protected` gana a `Generic`.** Un `expediente` dentro de `Backup` sigue
   siendo una frontera. Y una frontera tiene penalización 0: ser un límite no
   la convierte en mala ubicación canónica.

8. **`is_protected_boundary` se deriva del `kind`.** Estaba fijado a `0` en el
   `INSERT`; con el perfil jurídico eso habría dejado carpetas
   `kind='PROTECTED'` con el flag a `0` — un falso negativo de seguridad
   silencioso para cualquier lector de esa columna. Ahora hay un test que fija
   la coherencia entre ambos.

## Perfil jurídico

`profiles/legal/profile.json` declara como fronteras: expediente(s),
pericial(es), procedimiento(s), sumario, autos y asunto(s). El razonamiento
está en el propio archivo, por marcador: dos copias del mismo documento en
expedientes distintos **no son redundancia**, porque cada expediente debe
sostenerse por sí mismo como unidad probatoria (§15.2, §15.3
`ACROSS_PROTECTED_CONTEXTS`).

## Alternativas consideradas

- **Mantener los marcadores en código** — descartada: el criterio de qué es una
  frontera es conocimiento de dominio, y debe poder revisarlo un jurista sin
  tocar Rust.
- **Leer los perfiles del disco en runtime** — descartada por ahora: introduce
  resolución de rutas y una vía para alterar en silencio qué se consolida. Es
  el problema que resuelven los plugins con capacidades explícitas (M0.6).
- **YAML** — descartada por §5.7 y por la dependencia (ver decisión 3).

## Consecuencias

- La regla 9 pasa de correcta a **efectiva**: con `--profile legal`, un
  duplicado dentro de un expediente se conserva bajo cualquier política.
- Añadir un dominio (fotografía, migración) es escribir un JSON y probarlo.
- Deuda aceptada: la coincidencia es por nombre (`exact` o `prefix`), no por
  contenido ni por anclas fuertes. Un `Expediente 1234-2020` se protege por el
  prefijo, pero una carpeta llamada `Martínez vs. Ayuntamiento` que sea de
  facto un expediente no se detecta: eso necesita el grafo de entidades y las
  anclas del §18.2 (número de procedimiento, identificador fiscal), y el rule
  engine del §25.1.

## Tests

`profile::tests` (7): parseo de todos los perfiles, `generic` sin fronteras
(§25.4), fallback conservador, herencia, penalizaciones §18.3, y
`the_legal_profile_marks_expedientes_as_protected`.
`context::tests`: `the_legal_profile_turns_expedientes_into_boundaries` y
`the_protected_flag_always_agrees_with_the_kind`.
