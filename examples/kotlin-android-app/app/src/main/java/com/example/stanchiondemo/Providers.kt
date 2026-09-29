package com.example.stanchiondemo

import uniffi.stanchion_uniffi.CapabilityCall
import uniffi.stanchion_uniffi.CapabilityProvider
import uniffi.stanchion_uniffi.CapabilityRequest
import uniffi.stanchion_uniffi.Decision
import uniffi.stanchion_uniffi.Policy
import uniffi.stanchion_uniffi.ProviderException
import uniffi.stanchion_uniffi.Value

/** Answers the `locale` capability with the device's BCP-47 tag. */
class LocaleProvider(
    private val tag: () -> String,
) : CapabilityProvider {
    override fun invoke(call: CapabilityCall): Value {
        if (call.args.isNotEmpty()) {
            throw ProviderException.Refused("locale takes no arguments")
        }
        return Value.Str(tag())
    }
}

/**
 * Grants everything the bundled plugins ask for and records what it saw, so
 * the diagnostics card can show the policy was actually consulted. Denies the
 * `secret` capability nothing requests — the fail-closed default, kept visible.
 */
class AppPolicy : Policy {
    val seen = mutableListOf<CapabilityRequest>()

    override fun decide(request: CapabilityRequest): Decision {
        seen.add(request)
        return if (request.capability == "secret") {
            Decision.Deny("nothing in this app may read secrets")
        } else {
            Decision.Grant
        }
    }
}
