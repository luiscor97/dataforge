# Accesibilidad del escritorio

El escritorio (Tauri + React) presenta DTOs del motor; nunca decide. Su
diseño de accesibilidad sigue estos principios, verificados en la suite de
UI (`pnpm --filter dataforge-desktop test:ui`).

## Estructura y semántica

- **Idioma del documento**: `<html lang="es">`.
- **Landmarks**: `<main>` para el contenido, `<header>` para la barra
  superior. Cada bloque de diagnóstico es una `<section>` con
  `aria-labelledby` apuntando a su encabezado.
- **Jerarquía de encabezados**: `h1` (marca) → `h2` (pantalla) → `h3`
  (sección de diagnóstico) → `h4` (grupo), sin saltos.
- **Formularios**: cada campo va dentro de su `<label>` (asociación
  implícita); los campos obligatorios usan `required`. Los ajustes de
  contenido usan `htmlFor`/`id` explícitos.

## Estados dinámicos

- **Operaciones en curso**: `<main aria-busy>` se marca mientras hay una
  llamada asíncrona, y los botones de acción se deshabilitan y cambian su
  texto ("Creando…", "Analizando…"). La tecnología asistiva anuncia el
  estado de ocupación.
- **Errores**: se muestran en una región `role="alert"` (asertiva), que se
  anuncia en cuanto aparece.
- **Diagnósticos**: los estados pendiente/sellado usan `role="status"` y
  `aria-live` para anunciar el resultado sin robar el foco; los contadores
  parciales nunca se presentan como finales.

## Contenido no textual

- Los separadores decorativos (`↔`, iconos) llevan `aria-hidden`.
- No hay imágenes informativas sin texto alternativo en la UI del motor.

## Límites declarados

- Falta un pase manual con lector de pantalla real (NVDA/VoiceOver) y una
  auditoría automatizada (axe) integrada en CI; ambos quedan como refuerzo
  de release. La base semántica (landmarks, etiquetas, live regions, estado
  de ocupación e idioma) está cubierta y probada.
