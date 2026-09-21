# frozen_string_literal: true

# Lua plugins for Ruby applications.
#
# Plugins live in `<root>/<plugin>/plugin.toml` alongside their entry chunk, and are
# called by name:
#
#   require "stanchion"
#
#   host = Stanchion::Registry.new(allow: ["log"])
#   report = host.load("plugins/")
#   report[:failures].each { |f| warn "#{f[:plugin]}: #{f[:reason]}" }
#   puts host.call("greeter", "greet", "world")
#
# Ruby code can answer capabilities the plugins declare. A provider is anything
# responding to `call`, handed a Hash with `:plugin`, `:capability`, `:grant` and
# `:args`:
#
#   host = Stanchion::Registry.new(
#     capabilities: { "kv" => ->(c) { STORE.fetch(c[:args].first) } },
#   )
#
# Do not call back into the registry from a provider: it runs while that registry is
# locked, and the attempt raises Stanchion::ReentrantError rather than deadlocking.
#
# A plugin call holds the GVL for its duration, so it blocks the Ruby VM. The
# instruction and memory limits are what bound that, and they are on by default.
#
# See https://github.com/jalcine/stanchion/blob/main/docs/bindings.md.
module Stanchion
end

# Ruby derives the extension's entry point from the file name, so the library has to
# be called `stanchion.so` whatever Cargo named it. `build.sh` stages it here.
require "stanchion/stanchion"
