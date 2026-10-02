import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HexFormat;
import java.util.List;
import java.util.concurrent.Executor;
import java.util.stream.Stream;
import net.minecraft.SharedConstants;
import net.minecraft.commands.Commands;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.LayeredRegistryAccess;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.ReloadableServerResources;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.server.permissions.LevelBasedPermissionSet;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.item.ItemStack;

/**
 * Decodes Pumpkin-encoded bytes with the vanilla 26.3 stream codecs.
 * Input lines: "<kind>\t<label>\t<hex>", kind = item | container_set_content.
 */
public class VanillaItemOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Executor direct = Runnable::run;
        var resources = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource().fullResources()));
        LayeredRegistryAccess<RegistryLayer> layers = RegistryLayer.createRegistryAccess();
        List<Registry.PendingTags<?>> staticTags = TagLoader.loadTagsForExistingRegistries(resources, layers.getLayer(RegistryLayer.STATIC));
        RegistryAccess.Frozen worldCtx = layers.getAccessForLoading(RegistryLayer.WORLD);
        List<HolderLookup.RegistryLookup<?>> worldLookups = TagLoader.buildUpdatedLookups(worldCtx, staticTags);
        RegistryAccess.Frozen world = RegistryDataLoader.load(resources, worldLookups, RegistryDataLoader.WORLD_REGISTRIES, direct).get();
        List<HolderLookup.RegistryLookup<?>> dimLookups = Stream.concat(worldLookups.stream(), world.listRegistries()).toList();
        RegistryAccess.Frozen dims = RegistryDataLoader.load(resources, dimLookups, RegistryDataLoader.DIMENSION_REGISTRIES, direct).get();
        LayeredRegistryAccess<RegistryLayer> loaded = layers.replaceFrom(RegistryLayer.WORLD, world, dims);
        ReloadableServerResources managers = ReloadableServerResources.loadResources(
                resources, loaded, staticTags, FeatureFlags.DEFAULT_FLAGS, Commands.CommandSelection.DEDICATED,
                LevelBasedPermissionSet.ALL, direct, direct).get();
        managers.updateComponentsAndStaticRegistryTags();
        RegistryAccess access = loaded.compositeAccess();

        if (args[0].equals("gen")) { gen(access, args[1], args[2]); return; }
        if (args[0].equals("check")) { check(access, args[1], args[2]); return; }
        if (args[0].equals("packet")) { packet(access, args[1], args[2]); return; }
        int fail = 0, ok = 0;
        for (String line : Files.readAllLines(Path.of(args[0]))) {
            if (line.isBlank()) continue;
            String[] p = line.split("\t", 3);
            String kind = p[0], label = p[1];
            ByteBuf raw = Unpooled.wrappedBuffer(HexFormat.of().parseHex(p[2]));
            RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(raw, access);
            try {
                String shown;
                if (kind.equals("item")) {
                    ItemStack stack = ItemStack.OPTIONAL_STREAM_CODEC.decode(buf);
                    shown = stack + " " + stack.getComponentsPatch();
                } else {
                    ClientboundContainerSetContentPacket pkt = ClientboundContainerSetContentPacket.STREAM_CODEC.decode(buf);
                    shown = pkt.items().size() + " slots";
                }
                if (buf.readableBytes() > 0) {
                    throw new IllegalStateException(buf.readableBytes() + " bytes left over after decoding");
                }
                ok++;
                if (args.length > 1) System.out.println("OK   " + label + " -> " + shown);
            } catch (Throwable t) {
                fail++;
                Throwable root = t;
                while (root.getCause() != null) root = root.getCause();
                System.out.println("FAIL " + label + ": " + t + (root != t ? " | root: " + root : ""));
            }
        }
        System.out.println("decoded ok=" + ok + " fail=" + fail);
    }

    static ItemStack parse(RegistryAccess access, String spec) throws Exception {
        var ctx = net.minecraft.commands.CommandBuildContext.simple(access, FeatureFlags.DEFAULT_FLAGS);
        String[] parts = spec.split(" ", 2);
        int count = 1;
        String item = spec;
        if (parts.length == 2 && parts[0].matches("\\d+")) { count = Integer.parseInt(parts[0]); item = parts[1]; }
        return net.minecraft.commands.arguments.item.ItemArgument.item(ctx).parse(new com.mojang.brigadier.StringReader(item)).createItemStack(count);
    }

    /** cases file (one vanilla item string per line) -> label \t vanilla NBT (uncompressed, hex) \t vanilla network hex */
    static void gen(RegistryAccess access, String in, String out) throws Exception {
        var ops = access.createSerializationContext(net.minecraft.nbt.NbtOps.INSTANCE);
        StringBuilder sb = new StringBuilder();
        for (String spec : Files.readAllLines(Path.of(in))) {
            if (spec.isBlank() || spec.startsWith("#")) continue;
            try {
                ItemStack stack = parse(access, spec);
                var tag = (net.minecraft.nbt.CompoundTag) ItemStack.CODEC.encodeStart(ops, stack).getOrThrow();
                var bos = new java.io.ByteArrayOutputStream();
                net.minecraft.nbt.NbtIo.write(tag, new java.io.DataOutputStream(bos));
                RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), access);
                ItemStack.OPTIONAL_STREAM_CODEC.encode(buf, stack);
                byte[] net = new byte[buf.readableBytes()];
                buf.readBytes(net);
                RegistryFriendlyByteBuf dbuf = new RegistryFriendlyByteBuf(Unpooled.buffer(), access);
                ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC.encode(dbuf, stack);
                byte[] delimited = new byte[dbuf.readableBytes()];
                dbuf.readBytes(delimited);
                sb.append(spec).append('\t').append(HexFormat.of().formatHex(bos.toByteArray())).append('\t').append(HexFormat.of().formatHex(net)).append('\t').append(HexFormat.of().formatHex(delimited)).append('\n');
            } catch (Throwable t) {
                System.out.println("GENFAIL " + spec + ": " + t);
            }
        }
        Files.writeString(Path.of(out), sb.toString());
    }

    /** pumpkin output: label \t pumpkin network hex (or ERR msg). Decodes with vanilla and compares with the parsed spec. */
    static void check(RegistryAccess access, String in, String mode) throws Exception {
        int ok = 0, fail = 0;
        for (String line : Files.readAllLines(Path.of(in))) {
            if (line.isBlank()) continue;
            String[] p = line.split("\t", 2);
            String spec = p[0];
            try {
                if (p[1].startsWith("ERR")) throw new IllegalStateException("pumpkin: " + p[1]);
                ItemStack expected = parse(access, spec);
                RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.wrappedBuffer(HexFormat.of().parseHex(p[1])), access);
                ItemStack got = ItemStack.OPTIONAL_STREAM_CODEC.decode(buf);
                if (buf.readableBytes() > 0) throw new IllegalStateException(buf.readableBytes() + " bytes left over");
                if (!ItemStack.matches(expected, got)) throw new IllegalStateException("mismatch: expected " + expected.getComponentsPatch() + " got " + got.getComponentsPatch());
                ok++;
                if (mode.equals("v")) System.out.println("OK   " + spec);
            } catch (Throwable t) {
                fail++;
                Throwable root = t; while (root.getCause() != null) root = root.getCause();
                System.out.println("FAIL " + spec + " :: " + t + (root != t ? " | root: " + root : ""));
            }
        }
        System.out.println("checked ok=" + ok + " fail=" + fail);
    }

    /** inventory file: "<slot> <item spec>" per line -> slot/nbt lines plus the vanilla container_set_content body. */
    static void packet(RegistryAccess access, String in, String out) throws Exception {
        var ops = access.createSerializationContext(net.minecraft.nbt.NbtOps.INSTANCE);
        List<ItemStack> slots = new java.util.ArrayList<>(java.util.Collections.nCopies(46, ItemStack.EMPTY));
        StringBuilder sb = new StringBuilder();
        for (String line : Files.readAllLines(Path.of(in))) {
            if (line.isBlank() || line.startsWith("#")) continue;
            String[] p = line.split(" ", 2);
            int slot = Integer.parseInt(p[0]);
            ItemStack stack = parse(access, p[1]);
            slots.set(slot, stack);
            var tag = (net.minecraft.nbt.CompoundTag) ItemStack.CODEC.encodeStart(ops, stack).getOrThrow();
            var bos = new java.io.ByteArrayOutputStream();
            net.minecraft.nbt.NbtIo.write(tag, new java.io.DataOutputStream(bos));
            sb.append("slot\t").append(slot).append('\t').append(p[1]).append('\t').append(HexFormat.of().formatHex(bos.toByteArray())).append('\n');
        }
        RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), access);
        ClientboundContainerSetContentPacket.STREAM_CODEC.encode(buf, new ClientboundContainerSetContentPacket(0, 5, slots, ItemStack.EMPTY));
        byte[] bytes = new byte[buf.readableBytes()];
        buf.readBytes(bytes);
        sb.append("packet\t0\t5\t").append(HexFormat.of().formatHex(bytes)).append('\n');
        Files.writeString(Path.of(out), sb.toString());
    }
}
