// Compiles and runs the generated Kotlin bindings.
//
// Formatting the generated file proves it parses; only compiling proves it is valid
// Kotlin, and only running it proves JNA finds the library and the callback vtables
// work from the JVM. The Python harness next door covers the same semantics through
// the same scaffolding — this covers the parts that are Kotlin's alone.
//
// Run through `bindings/uniffi/smoke-kotlin.sh`.

import kotlinx.coroutines.runBlocking
import uniffi.stanchion_uniffi.CapabilityCall
import uniffi.stanchion_uniffi.CapabilityProvider
import uniffi.stanchion_uniffi.CapabilityRequest
import uniffi.stanchion_uniffi.Config
import uniffi.stanchion_uniffi.Decision
import uniffi.stanchion_uniffi.Policy
import uniffi.stanchion_uniffi.ProviderException
import uniffi.stanchion_uniffi.Stanchion
import uniffi.stanchion_uniffi.StanchionException
import uniffi.stanchion_uniffi.Value
import java.nio.file.Files
import java.nio.file.Path
import kotlin.io.path.createDirectories
import kotlin.io.path.writeText

val failures = mutableListOf<String>()

fun check(name: String, condition: Boolean, detail: String = "") {
    if (condition) {
        println("  ok   $name")
    } else {
        println("  FAIL $name $detail")
        failures.add(name)
    }
}

fun writePlugin(root: Path, name: String, manifest: String, source: String) {
    val directory = root.resolve(name)
    directory.createDirectories()
    directory.resolve("plugin.toml").writeText(manifest)
    directory.resolve("init.lua").writeText(source)
}

fun config() = Config(
    plugins = null,
    libs = null,
    deny = null,
    memoryLimit = null,
    instructionLimit = null,
    shared = false,
    requireSignatures = false,
    allow = listOf(),
)

const val ECHO = """
local P = {}
P.__index = P
function P.new(config) return setmetatable({ tag = config.tag or "echo" }, P) end
function P:identity(value) return value end
function P:tagged() return self.tag end
function P:boom() error("deliberate") end
return P
"""

const val CALLER = """
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:lookup(key) return kv(key) end
function P:attempt(key)
  local ok, err = pcall(function() return kv(key) end)
  return ok and err or ("refused: " .. tostring(err))
end
return P
"""

const val YIELDING = """
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:tagged() coroutine.yield() return "awake" end
return P
"""

/** A foreign capability implementation, called from inside Lua. */
class Store : CapabilityProvider {
    val seen = mutableListOf<CapabilityCall>()

    override fun invoke(call: CapabilityCall): Value {
        seen.add(call)
        val key = call.args.first()
        val name = (key as? Value.Str)?.value ?: "?"
        if (name == "boom") {
            throw ProviderException.Refused("no such key")
        }
        return Value.Str("value-of-$name")
    }
}

/** A foreign policy that narrows rather than answering yes or no. */
class Narrowing : Policy {
    val seen = mutableListOf<CapabilityRequest>()

    override fun decide(request: CapabilityRequest): Decision {
        seen.add(request)
        return Decision.GrantWith(Value.Table(mapOf("keys" to Value.Seq(listOf(Value.Str("alpha"))))))
    }
}

/** Calls back into the registry that invoked it. */
class Reenters : CapabilityProvider {
    var host: Stanchion? = null
    val caught = mutableListOf<Throwable>()

    override fun invoke(call: CapabilityCall): Value {
        try {
            host!!.call("caller", "lookup", listOf(Value.Str("again")))
        } catch (err: Throwable) {
            caught.add(err)
            throw ProviderException.Refused(err.message ?: "re-entry refused")
        }
        return Value.Nil
    }
}

fun valuesRoundTrip(root: Path) {
    println("values round-trip through the generated converters")
    writePlugin(root, "echo", "name = \"echo\"\nentry = \"init.lua\"\n", ECHO)
    val host = Stanchion(config(), mapOf(), null)
    host.load(root.toString())

    val cases = listOf(
        Value.Nil,
        Value.Bool(true),
        Value.Int(-9L),
        Value.Float(1.5),
        Value.Str("hello"),
        Value.Seq(listOf(Value.Int(1L), Value.Int(2L))),
        Value.Table(mapOf("k" to Value.Bool(false))),
    )
    for (case in cases) {
        val got = host.call("echo", "identity", listOf(case))
        check("identity $case", got == case, "got $got")
    }

    check("tagged", host.call("echo", "tagged", listOf()) == Value.Str("echo"))
    check("count", host.count() == 1UL)
    check("names", host.names() == listOf("echo"))
    check("isolation", host.isolation() == "per-plugin")
}

fun errorsMap(root: Path) {
    println("failures arrive as the right generated exception")
    writePlugin(root, "echo", "name = \"echo\"\nentry = \"init.lua\"\n", ECHO)
    val host = Stanchion(config(), mapOf(), null)
    host.load(root.toString())

    try {
        host.call("absent", "tagged", listOf())
        check("unknown plugin throws", false)
    } catch (err: StanchionException.UnknownPlugin) {
        check("unknown plugin throws", true)
    } catch (err: Throwable) {
        check("unknown plugin throws", false, "got ${err::class.simpleName}")
    }

    try {
        host.call("echo", "boom", listOf())
        check("lua error throws", false)
    } catch (err: StanchionException.Lua) {
        check("lua error throws", err.message!!.contains("deliberate"), err.message!!)
    } catch (err: Throwable) {
        check("lua error throws", false, "got ${err::class.simpleName}")
    }
}

fun capabilityCallback(root: Path) {
    println("a plugin reaches a capability implemented in Kotlin")
    writePlugin(root, "caller", "name = \"caller\"\nentry = \"init.lua\"\n\n[capabilities.kv]\n", CALLER)

    val store = Store()
    val host = Stanchion(config(), mapOf("kv" to store), null)
    val report = host.load(root.toString())
    check("caller loaded", report.loaded == listOf("caller"), report.failures.toString())

    val got = host.call("caller", "lookup", listOf(Value.Str("alpha")))
    check("callback returned", got == Value.Str("value-of-alpha"), "$got")
    check("provider saw one call", store.seen.size == 1)
    check("provider saw the plugin", store.seen.firstOrNull()?.plugin == "caller")

    val message = host.call("caller", "attempt", listOf(Value.Str("boom")))
    val text = (message as? Value.Str)?.value ?: ""
    check(
        "a throwing provider is catchable in Lua",
        text.contains("refused") && text.contains("no such key"),
        text,
    )
}

fun policyCallback(root: Path) {
    println("a policy written in Kotlin decides, and can narrow")
    writePlugin(
        root,
        "caller",
        "name = \"caller\"\nentry = \"init.lua\"\n\n[capabilities.kv]\nkeys = [\"*\"]\n",
        CALLER,
    )

    val store = Store()
    val policy = Narrowing()
    val host = Stanchion(config(), mapOf("kv" to store), policy)
    val report = host.load(root.toString())
    check("loaded under policy", report.loaded == listOf("caller"), report.failures.toString())
    check("policy was consulted", policy.seen.size == 1)
    check("policy saw the signer", policy.seen.firstOrNull()?.signer == "unsigned")

    host.call("caller", "lookup", listOf(Value.Str("alpha")))
    val expected = Value.Table(mapOf("keys" to Value.Seq(listOf(Value.Str("alpha")))))
    check(
        "the narrowed grant reached the provider",
        store.seen.firstOrNull()?.grant == expected,
        "${store.seen.firstOrNull()?.grant}",
    )
}

fun reentrancy(root: Path) {
    println("re-entering from a Kotlin callback is caught, not hung")
    writePlugin(root, "caller", "name = \"caller\"\nentry = \"init.lua\"\n\n[capabilities.kv]\n", CALLER)

    val provider = Reenters()
    val host = Stanchion(config(), mapOf("kv" to provider), null)
    provider.host = host
    host.load(root.toString())

    // Without the guard this never returns.
    try {
        host.call("caller", "lookup", listOf(Value.Str("k")))
        check("re-entry refused", false, "the call unexpectedly succeeded")
    } catch (err: StanchionException.Lua) {
        check("re-entry refused", true)
    } catch (err: Throwable) {
        check("re-entry refused", false, "got ${err::class.simpleName}")
    }
    check("the provider saw one failure", provider.caught.size == 1)
    check(
        "it was the reentrancy error",
        provider.caught.firstOrNull() is StanchionException.Reentrant,
        "${provider.caught.firstOrNull()?.let { it::class.simpleName }}",
    )
}

fun suspending(root: Path) = runBlocking {
    println("suspend functions cross the generated boundary")
    writePlugin(root, "sleeper", "name = \"sleeper\"\nentry = \"init.lua\"\n", YIELDING)
    val host = Stanchion(config(), mapOf(), null)
    host.load(root.toString())

    check("callAsync", host.callAsync("sleeper", "tagged", listOf()) == Value.Str("awake"))
    val outcomes = host.dispatchAsync("tagged", listOf())
    check("dispatchAsync", outcomes.map { it.value } == listOf(Value.Str("awake")), "$outcomes")
}

/** Runs one test against a throwaway plugin root. */
fun withRoot(test: (Path) -> Unit) {
    val directory = Files.createTempDirectory("stanchion-kt")
    try {
        test(directory)
    } finally {
        directory.toFile().deleteRecursively()
    }
}

fun main() {
    // Called directly rather than through a list of function references: those are
    // `KFunction`s, and resolving one needs `kotlin-reflect` on the classpath.
    withRoot(::valuesRoundTrip)
    withRoot(::errorsMap)
    withRoot(::capabilityCallback)
    withRoot(::policyCallback)
    withRoot(::reentrancy)
    withRoot(::suspending)

    println()
    if (failures.isNotEmpty()) {
        println("${failures.size} check(s) failed: ${failures.joinToString(", ")}")
        kotlin.system.exitProcess(1)
    }
    println("every check passed")
}
