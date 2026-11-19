<!-- translation-of: README.md @ blob 1f6e2c134bed3bf58252a9a0f86f648a5f34c7db (2026-11-20) -->
> Esta es una traducción del [README en inglés](README.md) del 2026-11-20. La versión inglesa es la de referencia. La traducción es automática y un segundo sistema de IA la ha cotejado con el original. Aún no la ha revisado ningún hablante nativo de español.

# Istesso

[English](README.md) · [日本語](README.ja.md) · [简体中文](README.zh-Hans.md) · [한국어](README.ko.md) · **Español**

Istesso muestra las aplicaciones que se ejecutan en tus otros equipos como ventanas normales en el que tienes delante. Para ello utiliza su propio canal cifrado. Tiene una sola regla: si percibes que trabajas en remoto, aunque sea un poco, la prueba ha fallado.

<!-- Imagen pendiente (docs/images/): detalle de una pantalla con tres ventanas de aspecto idéntico.
     Una aplicación se ejecuta aquí, otra en un equipo Linux y otra en un Mac. Las etiquetas van fuera de la imagen.
     Se usarán únicamente la aplicación de prueba de Istesso y contenido ficticio. -->

> [!IMPORTANT]
> Istesso está en fase de prueba. Su autor lo usa a diario para averiguar qué falta, y buena parte de ese trabajo está en ramas que aún no se han incorporado a `main`. No hay binarios para descargar. Tienes que compilar el programa a partir del código fuente. La tabla de abajo indica qué funciona hoy y en qué equipos.

## Qué es

Istesso está diseñado para que cada tarea se haga donde tenga menos latencia, siguiendo este orden:

1. Lo que se puede ejecutar en el equipo que tienes delante se ejecuta ahí. Solo se sincroniza su estado con los demás equipos.
2. Para trabajar con texto se usa una conexión directa por terminal.
3. Quedan las aplicaciones que necesitan una interfaz gráfica y se ejecutan en otro equipo. Solo esas se transmiten como vídeo, con una ventana por aplicación y el puntero dibujado en tu equipo.

La prueba tiene un criterio de fallo concreto: si puedes distinguir qué ventana es remota, no se cumple la regla. Se mide la ventana retransmitida y se compara con la misma aplicación ejecutada localmente en la misma pantalla. Además, una persona intenta distinguir a simple vista cuál es la ventana retransmitida. La regla se aplica a equipos conectados por cable en una red doméstica. Quedan fuera las conexiones por Wi-Fi y por internet.

El nombre viene de *l'istesso tempo*, una indicación musical que pide mantener el mismo tempo aunque cambie el compás.

## Qué funciona hoy

Cada columna indica dónde se ejecuta la aplicación y en qué dispositivo la ves.

| Función | Anfitrión Linux → Mac | Anfitrión Mac → Mac | Anfitrión Linux → visor Android XR |
|---|---|---|---|
| Emparejar dos equipos con un código de un solo uso | `Verified` 2026-11-16 ([registro](docs/milestones.md#m0-共通の土台) (japonés)) | `Trial` | `Trial` |
| Ver todos los equipos y sus aplicaciones en una sola lista | `Verified` 2026-11-16 ([registro](docs/milestones.md#m0-共通の土台) (japonés)) | `Trial` | `Planned` |
| Abrir una aplicación en un anfitrión y usarla en una ventana con imagen, teclado, puntero y desplazamiento | `Trial` | `Trial` | `Trial` |
| Copiar y pegar texto entre esa ventana y el equipo que tienes delante | `Trial` | `Trial` | `Planned` |
| Escribir en japonés con un método de entrada | `Trial` | `Trial` | `Trial` |
| Oír el sonido de un anfitrión en el lugar donde estás | `Planned` | `Planned` | `Planned` |
| Abrir una aplicación desde una ventana del lanzador, esté instalada donde esté | `Planned` | `Planned` | `Planned` |
| Ceder un dispositivo USB a otro equipo para que aparezca como si estuviera conectado físicamente a él | `Out of scope` | `Out of scope` | `Out of scope` |

- `Verified` (verificado) indica que se ha ejecutado desde `main` en equipos físicos que desempeñan las funciones indicadas, con los mismos pasos que seguiría quien lee esta guía. Siempre se incluyen la fecha y un enlace al registro.
- `Trial` (en pruebas) indica que funciona en la versión de prueba que el autor compila a partir de las ramas de desarrollo. Aún no puedes reproducirlo desde `main`.
- `Planned` (previsto) indica que está en fase de diseño o que el trabajo aún no ha empezado.
- `Out of scope` (fuera de alcance) indica que no forma parte de esta versión.

El visor se conecta por Wi-Fi, por lo que queda fuera de la regla de latencia, que se aplica a una red doméstica por cable. En su columna, `Trial` indica que la función se puede usar en la versión de prueba. No dice nada sobre el cumplimiento de la regla.

La latencia añadida por la retransmisión se mide respecto a la misma aplicación ejecutada localmente en la misma pantalla. Con ambos equipos conectados por cable, una pantalla de 85 Hz y una imagen de 3440×1440, no debe superar los 12 ms en la mediana, los 24 ms en el percentil 95 ni los 47 ms en el percentil 99. La retransmisión pasó esta prueba el 2026-11-17 con un prototipo del programa de visualización y un anfitrión preparado para las mediciones. La aceptación depende del resultado de la prueba final, que se hará con el cliente y el anfitrión propios de Istesso ([registro de la prueba](docs/measurements/2026-11-17-m1-gate.md) (japonés)).

## Qué vendrá después

- Incorporar el cliente y el anfitrión propios de Istesso a `main` y, después, emitir el veredicto final sobre la latencia.
- Enviar el sonido de cualquier anfitrión a una única salida de tu elección, durante horas y sin cortes.
- Abrir aplicaciones de varios equipos desde un lanzador que pueda despertar a un anfitrión en reposo.
- Compartir almacenamiento entre equipos y usar una cuenta de usuario dedicada para mantener las aplicaciones y los agentes de Istesso separados de tus archivos.
- Ejecutar compilaciones y trabajos en el equipo más adecuado para cada uno.
- Usar un dispositivo desde varios equipos sin moverlo. Una sola lista mostrará los dispositivos de todos los equipos. Si una aplicación necesita un dispositivo, se abrirá en el equipo al que esté conectado y podrás usar su ventana desde donde estés. El primer dispositivo que se controlará directamente será un teclado auxiliar programable. Como indica la tabla, ceder el propio dispositivo USB a otro equipo queda fuera de alcance.
- Incorporar el cliente para el visor Android XR a `main`.
- Ofrecer paquetes firmados para macOS y Ubuntu, proporcionar actualizaciones y someter el emparejamiento a una revisión externa.
- Añadir compatibilidad con Windows después de esta versión.

Cada etapa y la prueba que debe superar se describen en [docs/milestones.md](docs/milestones.md) (japonés).

## Limitaciones conocidas

- Istesso no está protegido frente a otros programas que se ejecuten con la misma cuenta de usuario en el mismo equipo. Úsalo en equipos propios en los que solo se ejecuten programas de confianza.
- La regla de latencia se aplica a una red doméstica por cable. Wi-Fi e internet quedan fuera de su alcance ([notas sobre Wi-Fi](docs/wifi.md) (japonés)).
- Un anfitrión Linux necesita una GPU NVIDIA con NVENC, su codificador de vídeo por hardware.
- Tras reiniciar un Mac anfitrión, no se puede usar como anfitrión hasta que alguien inicie sesión una vez en ese Mac.
- Cuando un Mac actúa como anfitrión, macOS pide confirmación en la pantalla de ese Mac cada 30 días para que la aplicación pueda seguir capturando la pantalla sin usar el selector del sistema. Es una regla de Apple para las aplicaciones que capturan la pantalla de esta manera. Istesso no la añade y la confirmación se pide también en las versiones compiladas a partir del código fuente. No se distribuyen versiones firmadas y aún no se ha decidido cómo se distribuirá Istesso.
- No hay instalador, actualizaciones automáticas ni binarios firmados. El emparejamiento aún no se ha sometido a una revisión externa.
- Las cifras de latencia disponibles hasta ahora proceden de un prototipo del programa de visualización. La aceptación queda pendiente de la prueba final.
- Esta versión no es compatible con Windows.

## En qué se diferencia de herramientas similares

| Tipo de herramienta | Qué ofrece | Cómo está diseñado Istesso | Cuándo conviene más esa herramienta |
|---|---|---|---|
| Escritorio remoto (VNC, RDP y similares) | El escritorio completo de otro equipo, incluida la pantalla de inicio de sesión, también a través de internet | Cada aplicación aparece en una ventana de tu escritorio, junto a tus ventanas locales y las de otros equipos | Necesitas el escritorio completo, la pantalla de inicio de sesión, acceso por internet o Windows |
| Terminal por ssh con un multiplexor de terminal | Trabajo con texto con la menor latencia posible en cualquier conexión y sesiones que se mantienen aunque se corte la conexión | Istesso no lo sustituye. El trabajo con texto por terminal es la segunda de sus tres capas. El vídeo se reserva para las aplicaciones que necesitan una interfaz gráfica | Trabajas con texto |
| Reenvío de ventanas (por ejemplo, X11 por ssh) | Ventanas individuales de aplicaciones X11 remotas en tu pantalla a través de una conexión ssh | Cada ventana se envía como vídeo codificado por hardware y se mide en comparación con una ventana local | Usas aplicaciones X11 y solo quieres configurar una conexión ssh |
| Transmisión de videojuegos | Pantalla completa, movimiento relativo del ratón, mandos de juego y partidas por internet | Está pensado para ventanas de aplicaciones. Usa posiciones absolutas del puntero y dibuja el cursor en tu equipo. Los juegos quedan fuera de esta versión | Quieres jugar |

## Inicio rápido

Estos pasos se realizan en un solo equipo. Solo muestran lo que Istesso encuentra en él: las funciones que puede desempeñar, las aplicaciones instaladas y los dispositivos conectados. Este README aún no incluye los pasos para conectar dos equipos y mostrar una aplicación remota porque todavía no se han vuelto a ejecutar desde un clon limpio del repositorio.

Necesitas `git`, Rust 1.85 o posterior y una cadena de herramientas de C (en macOS, las Xcode Command Line Tools).

```sh
git clone https://github.com/maromocooo/Istesso.git
cd Istesso
cargo build --release --locked -p istesso-cli
./target/release/istesso doctor
./target/release/istesso apps list
./target/release/istesso devices list
```

- `doctor` indica qué funciones puede desempeñar el equipo. Por ejemplo, si tiene un codificador de vídeo por hardware, si puede compilar para plataformas de Apple o si es adecuado como anfitrión siempre encendido. También muestra el nombre del equipo y sus direcciones de red. Revisa la salida antes de pegarla en otro sitio.
- `apps list` muestra las aplicaciones instaladas en este equipo.
- `devices list` muestra los dispositivos conectados a este equipo.

La última vez que se ejecutaron estos pasos desde un clon limpio fue el 2026-11-19, en macOS 26.5 (Apple silicon) ([registro](docs/readme-checks.md) (japonés)).

## Compilar a partir del código fuente

- Se necesita Rust 1.85 o posterior. Istesso se compila en macOS y Linux. La integración continua (CI) compila y prueba cada cambio en ambos sistemas.
- No se publican binarios firmados y aún no se ha decidido cómo se distribuirá Istesso.

Antes de enviar un cambio, ejecuta los siguientes comandos. La CI realiza las mismas comprobaciones.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Para ejecutar `cargo deny check` necesitas [cargo-deny](https://github.com/EmbarkStudios/cargo-deny).

## Seguridad

- Al emparejar dos equipos, uno muestra un código de un solo uso de ocho caracteres y tú lo introduces en el otro. Ambos usan el código para realizar un intercambio de claves autenticado mediante contraseña (PAKE) con SPAKE2. Luego intercambian sus claves de larga duración y cada equipo guarda como fija la clave del otro ([ADR 0006](docs/adr/0006-pairing.md) (japonés)).
- En cada conexión posterior, ambos extremos se autentican con las claves fijadas y la conexión se cifra mediante el marco de protocolos Noise. La imagen se transmite cifrada con las mismas claves ([ADR 0007](docs/adr/0007-control-channel.md), [ADR 0015](docs/adr/0015-media-transport.md) (japonés)).
- Un equipo recién emparejado solo tiene permisos de visualización y control. Cada permiso puede revocarse en el equipo que lo concedió.

El emparejamiento aún no se ha sometido a una revisión externa. Está previsto hacerla antes de publicar una versión. Istesso no está protegido frente a otros programas que se ejecuten con la misma cuenta de usuario en el mismo equipo.

Para informar de una vulnerabilidad, usa el canal privado de GitHub. En la pestaña Security, selecciona «Report a vulnerability». No abras una incidencia pública para comunicarla.

## Documentación

Los documentos más extensos están en japonés.

- [Requisitos](docs/requirements.md) (japonés)
- [Hitos y sus pruebas de aceptación](docs/milestones.md) (japonés)
- [Arquitectura](docs/architecture.md) (japonés)
- [Decisiones de arquitectura (ADR)](docs/adr/) (japonés)
- [Mediciones](docs/measurements/) (japonés)
- [Componentes y sus licencias](docs/components.md) (japonés)

## Cómo contribuir

- Las reglas de trabajo para las personas y los agentes de programación están en [AGENTS.md](AGENTS.md) (japonés). Trabaja en una rama y añade pruebas para los comportamientos nuevos.
- No se incorpora código GPL ni AGPL. Antes de usar código externo, comprueba si está permitido según [docs/components.md](docs/components.md) (japonés). Antes de adoptar una especificación o un protocolo, comprueba tanto las condiciones de sus patentes como su licencia. Si algo no está claro, se deja fuera.
- Si un cambio afecta al recorrido del vídeo, la entrada o el sonido, incluye en la solicitud de incorporación de cambios las mediciones de antes y después, o indica que no se pudo medir.
- Aún no se aceptan solicitudes de incorporación que añadan adaptadores de dispositivos porque su interfaz no está definida ([ADR 0012](docs/adr/0012-device-framework.md) (japonés)).

## Licencia

Puedes usar Istesso bajo la MIT License o la Apache License 2.0, a tu elección. Consulta [LICENSE-MIT](LICENSE-MIT) y [LICENSE-APACHE](LICENSE-APACHE).

## Autor

[maromocooo](https://github.com/maromocooo)

## Marcas comerciales

Apple, Mac, macOS y Xcode son marcas comerciales de Apple Inc. Ubuntu es una marca comercial de Canonical Ltd. Linux es una marca registrada de Linus Torvalds. NVIDIA y NVENC son marcas comerciales de NVIDIA Corporation. Android es una marca comercial de Google LLC. Rust es una marca comercial de la Rust Foundation. Istesso no está afiliado a ninguno de estos titulares ni cuenta con su respaldo.
