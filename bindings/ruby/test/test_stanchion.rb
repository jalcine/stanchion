# frozen_string_literal: true

# The Ruby binding, driven the way a Ruby application drives it.
#
# These mirror `bindings/python/tests` and `bindings/uniffi/tests` deliberately: all
# three sit on the same seam and are supposed to behave alike, so a divergence here
# is a bug in one of them rather than a difference worth keeping.

require "minitest/autorun"
require "tmpdir"
require "stanchion"

ECHO = <<~LUA
  local P = {}
  P.__index = P
  function P.new(config) return setmetatable({ tag = config.tag or "echo" }, P) end
  function P:identity(value) return value end
  function P:tagged() return self.tag end
  function P:sum(a, b) return a + b end
  function P:shape() return { n = 1, xs = {"a", "b"} } end
  function P:boom() error("deliberate") end
  return P
LUA

CALLER = <<~LUA
  local P = {}
  P.__index = P
  function P.new(_) return setmetatable({}, P) end
  function P:lookup(key) return kv(key) end
  function P:attempt(key)
    local ok, err = pcall(function() return kv(key) end)
    return ok and err or ("refused: " .. tostring(err))
  end
  return P
LUA

YIELDING = <<~LUA
  local P = {}
  P.__index = P
  function P.new(config) return setmetatable({ tag = config.tag or "awake" }, P) end
  function P:tagged() coroutine.yield() return self.tag end
  function P:double(n) coroutine.yield() return n * 2 end
  return P
LUA

module PluginFixtures
  def write_plugin(root, name, manifest, source)
    dir = File.join(root, name)
    Dir.mkdir(dir) unless Dir.exist?(dir)
    File.write(File.join(dir, "plugin.toml"), manifest)
    File.write(File.join(dir, "init.lua"), source)
  end

  def with_plugin(name, manifest, source)
    Dir.mktmpdir do |root|
      write_plugin(root, name, manifest, source)
      yield root
    end
  end

  def echo_host
    with_plugin("echo", %(name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "first"\n), ECHO) do |root|
      host = Stanchion::Registry.new
      report = host.load(root)
      assert report[:clean], report[:failures].inspect
      yield host, root
    end
  end

  CALLER_MANIFEST = %(name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n)
end

class TestRegistry < Minitest::Test
  include PluginFixtures

  def test_a_plugin_loads_and_answers
    echo_host do |host|
      assert_equal ["echo"], host.names
      assert_equal 1, host.length
      assert_equal "first", host.call("echo", "tagged")
      assert_equal 5, host.call("echo", "sum", 2, 3)
    end
  end

  def test_values_survive_the_boundary_in_both_directions
    echo_host do |host|
      assert_nil host.call("echo", "identity", nil)
      [true, false, 7, -9, 1.5, "hello", [1, 2, 3], { "k" => false }, [],
       { "a" => [1, { "b" => 2 }] }].each do |value|
        assert_equal value, host.call("echo", "identity", value), "round-tripping #{value.inspect}"
      end
    end
  end

  def test_booleans_do_not_collapse
    echo_host do |host|
      # Everything but nil and false is truthy in Ruby; a naive conversion would turn
      # every string into `true`.
      assert_equal true, host.call("echo", "identity", true)
      assert_equal false, host.call("echo", "identity", false)
      assert_equal "yes", host.call("echo", "identity", "yes")
    end
  end

  def test_a_symbol_arrives_as_a_string
    echo_host do |host|
      # Lua has nothing to call a symbol, so it crosses as a string.
      assert_equal "alpha", host.call("echo", "identity", :alpha)
    end
  end

  def test_a_symbol_keyed_hash_arrives_with_string_keys
    echo_host do |host|
      assert_equal({ "a" => 1 }, host.call("echo", "identity", { a: 1 }))
    end
  end

  def test_a_table_comes_back_as_a_hash
    echo_host do |host|
      assert_equal({ "n" => 1, "xs" => %w[a b] }, host.call("echo", "shape"))
    end
  end

  def test_an_unknown_plugin_raises_its_own_error
    echo_host do |host|
      assert_raises(Stanchion::UnknownPluginError) { host.call("absent", "tagged") }
    end
  end

  def test_a_raising_method_becomes_a_lua_error
    echo_host do |host|
      err = assert_raises(Stanchion::LuaError) { host.call("echo", "boom") }
      assert_includes err.message, "deliberate"
    end
  end

  def test_an_unconvertible_argument_is_refused
    echo_host do |host|
      assert_raises(TypeError) { host.call("echo", "identity", Object.new) }
    end
  end

  def test_one_bad_plugin_does_not_stop_the_others
    Dir.mktmpdir do |root|
      write_plugin(root, "good", %(name = "good"\nentry = "init.lua"\n), ECHO)
      write_plugin(root, "bad", %(name = "bad"\nentry = "init.lua"\n), "not lua (((")

      report = Stanchion::Registry.new.load(root)
      assert_equal ["good"], report[:loaded]
      refute report[:clean]
      assert_equal ["bad"], report[:failures].map { |f| f[:plugin] }
    end
  end

  def test_a_dispatch_collects_every_plugin
    Dir.mktmpdir do |root|
      { "a" => "alpha", "b" => "beta" }.each do |name, tag|
        write_plugin(root, name,
                     %(name = "#{name}"\nentry = "init.lua"\n\n[config]\ntag = "#{tag}"\n), ECHO)
      end
      host = Stanchion::Registry.new
      host.load(root)

      assert_equal %w[alpha beta], host.dispatch("tagged").map { |o| o[:value] }.sort
      # A failing method is reported in place rather than raised.
      assert host.dispatch("boom").all? { |o| o[:error] }
    end
  end

  def test_a_reload_picks_up_new_source
    with_plugin("echo", %(name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "before"\n),
                ECHO) do |root|
      host = Stanchion::Registry.new
      host.load(root)
      assert_equal "before", host.call("echo", "tagged")

      File.write(File.join(root, "echo", "plugin.toml"),
                 %(name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "after"\n))
      host.reload("echo")
      assert_equal "after", host.call("echo", "tagged")
    end
  end

  def test_a_configured_root_is_used_when_none_is_passed
    with_plugin("echo", %(name = "echo"\nentry = "init.lua"\n), ECHO) do |root|
      host = Stanchion::Registry.new(plugins: root)
      assert host.load[:clean]
      assert_equal ["echo"], host.names
    end
  end

  def test_loading_without_any_root_says_so
    assert_raises(Stanchion::ConfigError) { Stanchion::Registry.new.load }
  end

  def test_an_unknown_standard_library_is_a_configuration_error
    err = assert_raises(Stanchion::ConfigError) { Stanchion::Registry.new(libs: ["sorcery"]) }
    assert_includes err.message, "sorcery"
  end

  def test_isolation_is_per_plugin_unless_asked_otherwise
    assert_equal "per-plugin", Stanchion::Registry.new.isolation
    assert_equal "shared", Stanchion::Registry.new(shared: true).isolation
  end

  def test_a_listing_reports_what_was_granted
    echo_host do |host|
      info = host.list.first
      assert_equal "echo", info[:name]
      assert_equal "unsigned", info[:signer]
      assert_empty info[:granted]
    end
  end

  def test_an_audit_does_not_run_the_plugin
    with_plugin("caller", PluginFixtures::CALLER_MANIFEST,
                "error('this plugin must never be executed')") do |root|
      entry = Stanchion::Registry.new.audit(root).first
      assert_equal "caller", entry[:plugin]
      assert_equal ["kv"], entry[:capabilities]
    end
  end
end

class TestCapabilities < Minitest::Test
  include PluginFixtures

  def test_a_plugin_reaches_a_ruby_capability
    seen = []
    provider = lambda do |call|
      seen << call
      "value-of-#{call[:args].first}"
    end

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => provider })
      assert host.load(root)[:clean]

      assert_equal "value-of-alpha", host.call("caller", "lookup", "alpha")
      assert_equal 1, seen.length
      assert_equal "caller", seen.first[:plugin]
      assert_equal "kv", seen.first[:capability]
      assert_equal ["alpha"], seen.first[:args]
    end
  end

  def test_a_provider_may_be_any_object_responding_to_call
    store = Class.new do
      def call(_call) = { "a" => [1, true] }
    end.new

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => store })
      host.load(root)
      assert_equal({ "a" => [1, true] }, host.call("caller", "lookup", "k"))
    end
  end

  def test_a_raising_provider_is_catchable_inside_lua
    provider = ->(_call) { raise KeyError, "no such key" }

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => provider })
      host.load(root)

      message = host.call("caller", "attempt", "k")
      assert_includes message, "refused"
      assert_includes message, "no such key"
    end
  end

  def test_a_capability_nobody_provides_keeps_the_plugin_out
    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      # No provider and no allow-list: the default policy denies.
      report = Stanchion::Registry.new.load(root)
      assert_empty report[:loaded]
      assert_equal 1, report[:failures].length
    end
  end

  def test_a_policy_can_narrow_what_a_plugin_asked_for
    seen = []
    provider = lambda do |call|
      seen << call
      "ok"
    end
    policy = lambda do |request|
      assert_equal "kv", request[:capability]
      assert_equal({ "keys" => ["*"] }, request[:params])
      assert_equal "unsigned", request[:signer]
      Stanchion::Decision.grant_with({ "keys" => ["alpha"] })
    end

    manifest = %(name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\nkeys = ["*"]\n)
    with_plugin("caller", manifest, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => provider }, policy: policy)
      host.load(root)
      host.call("caller", "lookup", "alpha")

      # The provider must see what policy approved, not what the manifest asked for.
      assert_equal({ "keys" => ["alpha"] }, seen.first[:grant])
    end
  end

  def test_a_denying_policy_keeps_the_plugin_out
    policy = ->(_request) { Stanchion::Decision.deny("not in this deployment") }

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => ->(_c) {} }, policy: policy)
      report = host.load(root)

      assert_empty report[:loaded]
      assert_includes report[:failures].first[:reason], "not in this deployment"
    end
  end

  def test_a_policy_that_raises_denies_rather_than_crashing
    policy = ->(_request) { raise "the policy is broken" }

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => ->(_c) {} }, policy: policy)
      report = host.load(root)

      assert_empty report[:loaded]
      assert_includes report[:failures].first[:reason], "the policy is broken"
    end
  end

  def test_a_policy_returning_the_wrong_type_denies
    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => ->(_c) {} },
                                     policy: ->(_r) { "yes please" })
      report = host.load(root)

      assert_empty report[:loaded]
      assert_includes report[:failures].first[:reason], "must return a Stanchion::Decision"
    end
  end

  def test_revoking_defangs_a_live_plugin
    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => ->(_c) { "ok" } })
      host.load(root)

      assert_equal "ok", host.call("caller", "lookup", "k")
      assert_equal true, host.revoke("caller", "kv")
      assert_raises(Stanchion::LuaError) { host.call("caller", "lookup", "k") }
      assert_equal false, host.revoke("caller", "kv")
    end
  end

  def test_a_provider_that_re_enters_gets_an_error_rather_than_a_hang
    # A provider runs while the registry is locked. Calling back in would hang the
    # process with no error and no stack, so it raises instead.
    caught = []
    host = nil
    provider = lambda do |_call|
      begin
        host.call("caller", "lookup", "again")
      rescue StandardError => e
        caught << e
        raise
      end
    end

    with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |root|
      host = Stanchion::Registry.new(capabilities: { "kv" => provider })
      host.load(root)

      assert_raises(Stanchion::LuaError) { host.call("caller", "lookup", "k") }
      assert_equal 1, caught.length
      assert_instance_of Stanchion::ReentrantError, caught.first
    end
  end

  def test_two_registries_may_nest
    # Different locks, so there is no hazard and this stays allowed.
    with_plugin("echo", %(name = "echo"\nentry = "init.lua"\n), ECHO) do |inner_root|
      inner = Stanchion::Registry.new
      inner.load(inner_root)

      with_plugin("caller", PluginFixtures::CALLER_MANIFEST, CALLER) do |outer_root|
        outer = Stanchion::Registry.new(
          capabilities: { "kv" => ->(_c) { inner.call("echo", "tagged") } },
        )
        outer.load(outer_root)
        assert_equal "echo", outer.call("caller", "lookup", "k")
      end
    end
  end
end

class TestAsync < Minitest::Test
  include PluginFixtures

  def test_call_async_drives_a_yielding_method
    with_plugin("sleeper", %(name = "sleeper"\nentry = "init.lua"\n), YIELDING) do |root|
      host = Stanchion::Registry.new
      host.load(root)

      # The plain call cannot drive a coroutine; this is what the async pair buys in
      # Ruby, where neither frees the VM.
      assert_raises(Stanchion::LuaError) { host.call("sleeper", "tagged") }
      assert_equal "awake", host.call_async("sleeper", "tagged")
      assert_equal 42, host.call_async("sleeper", "double", 21)
    end
  end

  def test_dispatch_async_collects_every_plugin
    Dir.mktmpdir do |root|
      { "a" => "alpha", "b" => "beta" }.each do |name, tag|
        write_plugin(root, name,
                     %(name = "#{name}"\nentry = "init.lua"\n\n[config]\ntag = "#{tag}"\n),
                     YIELDING)
      end
      host = Stanchion::Registry.new
      host.load(root)

      assert_equal %w[alpha beta], host.dispatch_async("tagged").map { |o| o[:value] }.sort
    end
  end

  def test_an_unknown_plugin_raises_on_the_async_path_too
    assert_raises(Stanchion::UnknownPluginError) do
      Stanchion::Registry.new.call_async("absent", "tagged")
    end
  end
end
