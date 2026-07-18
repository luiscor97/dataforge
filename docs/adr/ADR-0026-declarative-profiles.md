# ADR-0026 — Perfiles declarativos y fronteras protegidas

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 §18.3, §25.2, §25.4, §15.2, regla 9; ADR-0024, ADR-0025, ADR-0028

**Revisada:** 2026-07-16 para documentar el rechazo de perfiles desconocidos
y la ampliación versionada del esquema con reglas declarativas seguras.

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
   con `serde_json`, y una dependencia nueva no compensa. ADR-0028 mantiene
   JSON al añadir las reglas al esquema `1.1.0`.

4. **Herencia de un solo nivel** (`inherits`). `legal` reutiliza los
   contenedores genéricos de `generic` y añade sus fronteras. Más de un nivel
   se rechaza: la resolución debe ser trivial de auditar.

5. **Un perfil desconocido se rechaza.** `generic` solo es el valor por defecto
   cuando el usuario no selecciona perfil. Un id explícito que no exista —por
   ejemplo, un typo de `legal`— falla al crear, abrir y analizar el proyecto;
   caer a `generic` retiraría silenciosamente fronteras que el usuario creía
   activas.

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

9. **El esquema `1.1.0` admite reglas ordenadas.** Cada perfil puede añadir
   reglas versionadas de metadatos, resueltas junto con la herencia. Sus
   predicados, acciones seguras y evidencia se fijan en ADR-0028; esta
   ampliación no permite a un perfil borrar ni sobrescribir.

## Perfil jurídico

`profiles/legal/profile.json` declara como fronteras: expediente(s),
pericial(es), procedimiento(s), sumario, autos, asunto(s), cliente(s),
persona(s) y correspondencia. El razonamiento está en el propio archivo, por
marcador: dos copias del mismo documento en expedientes distintos **no son
redundancia**, porque cada expediente debe sostenerse por sí mismo como unidad
probatoria (§15.2, §15.3 `ACROSS_PROTECTED_CONTEXTS`).

## Alternativas consideradas

- **Mantener los marcadores en código** — descartada: el criterio de qué es una
  frontera es conocimiento de dominio, y debe poder revisarlo un jurista sin
  tocar Rust.
- **Leer los perfiles del disco en runtime** — descartada por ahora: introduce
  resolución de rutas y una vía para alterar en silencio qué se consolida. Es
  el problema que resuelven los plugins con capacidades explícitas (M0.6).
- **YAML** — descartada por §5.7 y por la dependencia (ver decisión 3).
- **Fallback de un id desconocido a `generic`** — descartado: convierte un
  error de configuración en pérdida silenciosa de protección.

## Consecuencias

- La regla 9 pasa de correcta a **efectiva**: con `--profile legal`, un
  duplicado dentro de un expediente se conserva bajo cualquier política.
- Añadir un dominio integrado es escribir un JSON, validar su esquema y
  probarlo; los perfiles de usuario en runtime siguen reservados a plugins.
- Deuda aceptada: la coincidencia de fronteras es por nombre (`exact` o
  `prefix`), no por contenido. Una carpeta llamada `Martínez vs.
  Ayuntamiento` que sea de facto un expediente no se detecta con estas
  señales, y M0.2 no pretende inferirlo.

## Tests

Las pruebas cubren parseo y validación de todos los perfiles, rechazo de ids
desconocidos, `generic` sin fronteras (§25.4), herencia de marcadores y reglas,
penalizaciones §18.3, prefijos acotados, fronteras jurídicas y coherencia entre
`kind` e `is_protected_boundary`.
