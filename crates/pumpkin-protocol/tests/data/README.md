# Vanilla item stack fixtures

These files are used by `../vanilla_item_encoding.rs`. They were produced by the
vanilla 26.3 game, so they are a reference that does not depend on Pumpkin's own
reader and writer agreeing with each other.

| File | Content |
|:--|:--|
| `vanilla_26_3_item_stacks.input` | One item per line in `/give` syntax (`[count ]item[components]`). |
| `vanilla_26_3_item_stacks.tsv` | Per item: the item as vanilla saves it (uncompressed NBT), the bytes vanilla writes for it with `ItemStack.OPTIONAL_STREAM_CODEC`, and with the length-prefixed `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`. |
| `vanilla_26_3_container_set_content.input` | A player inventory, one `<slot> <item>` per line. |
| `vanilla_26_3_container_set_content.tsv` | The saved NBT of each slot and the `ClientboundContainerSetContentPacket` body vanilla writes for that 46-slot inventory (window 0, state id 5, nothing carried). |
| `VanillaItemOracle.java` | The program that wrote the `.tsv` files. |

## Regenerating

The program runs against the unobfuscated 26.3 client jar and its libraries. It
bootstraps the vanilla registries from the built-in data pack, parses each item
with the game's own item argument parser and encodes it with the game's codecs.

1. Download the 26.3 client jar and the libraries listed in its version JSON
   (`https://piston-meta.mojang.com/mc/game/version_manifest_v2.json`). Build a
   classpath of all library jars (natives excluded) plus the client jar.
2. Compile and run it with Java 25:

   ```sh
   javac -cp "$CP" -d out VanillaItemOracle.java
   java -cp "$CP:out" VanillaItemOracle gen vanilla_26_3_item_stacks.input vanilla_26_3_item_stacks.tsv
   java -cp "$CP:out" VanillaItemOracle packet vanilla_26_3_container_set_content.input vanilla_26_3_container_set_content.tsv
   ```

   Then put the comment header back at the top of each `.tsv`.

`VanillaItemOracle check <file> q` goes the other way: it reads lines of
`<item>\t<hex>` where the hex is what Pumpkin wrote, decodes them with the
vanilla codec, and reports items vanilla cannot decode or decodes to something
else. That is how the encoding bugs these fixtures cover were found.
