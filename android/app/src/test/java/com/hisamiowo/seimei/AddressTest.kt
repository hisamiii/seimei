package com.hisamiowo.seimei

import org.junit.Assert.assertEquals
import org.junit.Test

class AddressTest {
    private fun ok(typed: String) = (address(typed) as Address.Ok).url

    @Test fun origins() {
        assertEquals("https://seimei.example.org", ok("seimei.example.org"))
        assertEquals("https://seimei.example.org", ok("  Seimei.Example.org/ "))
        assertEquals("https://seimei.example.org", ok("https://seimei.example.org/chat?x=1#y"))
        assertEquals("https://seimei.example.org:8443", ok("seimei.example.org:8443"))
        assertEquals("https://seimei.example.org", ok("https://seimei.example.org:443"))
        assertEquals("https://seimei.example.org", ok("https://me:pw@seimei.example.org"))
        assertEquals("http://localhost:8431", ok("http://localhost:8431"))
        assertEquals("http://127.0.0.1:8431", ok("http://127.0.0.1:8431/"))
        assertEquals("seimei.example.org", (address("seimei.example.org") as Address.Ok).shown)
    }

    @Test fun refusals() {
        assertEquals(Address.Bad(HTTPS_ONLY), address("http://192.168.1.20:8417"))
        assertEquals(Address.Bad(HTTPS_ONLY), address("http://seimei.example.org"))
        assertEquals(Address.Bad("Type the server's address."), address("   "))
        assertEquals(Address.Bad("That is not a server address."), address("ftp://seimei.example.org"))
        assertEquals(Address.Bad("That is not a server address."), address("not a host"))
    }
}
