# Factory presets

Presets that ship inside Lemon Studio. `build.rs` compiles every `.ospreset` file here into the program; they show
in each plugin window's Presets box (under **Factory**), in the ◀ ▶ order after the user's own presets, and in the
browser under **Plugin presets › <plugin> › Factory**. They are read-only: users cannot rename or delete them.

Spec: `OpenStudio-specs/plugins/presets.md` ("Factory presets", "File format").

## Layout

```
factory-presets/
  <plugin name>/            the name the host shows, e.g. "Lemon Trio" (characters Windows refuses become "_")
    <preset name>.ospreset
    <category>/             one folder level for categories; deeper folders are ignored
      <preset name>.ospreset
```

## Writing a preset by hand: the parameter-list form

```
OSPRESET 1
plugin=Lemon Reverb	id=clap:<plugin id>	format=clap	body=params
# Comments and blank lines are skipped.
Decay time = 25
High cut = 40
```

- Line 1 is exactly `OSPRESET 1`.
- Line 2 is tab-separated `key=value` pairs. `plugin` is the plugin's shown name. `id` is its identity
  (`Descriptor::uid`, e.g. `clap:org.lemonstudio.lemon-reverb`, `vst3:<32 hex digits>`). An empty `id` matches any
  plugin. `body=params` marks a parameter list.
- Each further line is `<parameter name> = <value>`. The name is the parameter's name as the plugin reports it (the
  browser's Current project › Effects/Generators › plugin lists them). The value is in the range the plugin reports
  to the host, which is not always the range its knob shows. The Lemon plugins are nih-plug plugins: a stepped
  (integer or choice) parameter's value is its step count from the minimum (0 = the lowest setting), and any other
  parameter is 0..1 normalized (a -100%..100% knob at -79% is 0.105). On load, every parameter the user can set first
  goes back to its default, then the listed ones are set. A name the plugin lacks is reported in the hint bar and
  skipped.

A preset may also hold the plugin's whole saved state (no `body=` key, the state's bytes after line 2). That is what
"Save preset as..." writes. It is what to use for settings that are not parameters, such as Lemon Sampler's sample.

The test `presets::tests::factory_presets_are_listed_and_read_by_path` checks that every file here decodes, that its
`plugin` matches its folder, and that a parameter list parses.
`presets::tests::factory_presets_of_built_in_plugins_name_their_parameters` checks, for the plugins compiled into the
program, that every listed name is a parameter and every value is inside its reported range.

## Names

Use plain, descriptive names ("Warm pad", "Large hall", "Punchy kick"). Do not use product names, artist names or
trademarks.
