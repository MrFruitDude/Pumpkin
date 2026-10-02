// Oracle for offline_uuids.tsv: the exact algorithm vanilla's
// net.minecraft.core.UUIDUtil.createOfflinePlayerUUID uses.
// Regenerate with: java OfflineUuidOracle.java > offline_uuids.tsv
import java.nio.charset.StandardCharsets;
import java.util.UUID;

public class OfflineUuidOracle {
    public static void main(String[] args) {
        String[] names = {"Notch", "jeb_", "Dinnerbone", "Steve", "Alex", "MrFruitDude", "a", "player_1234567890", "Ünïcødé"};
        for (String name : names) {
            UUID id = UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(StandardCharsets.UTF_8));
            System.out.println(name + "\t" + id);
        }
    }
}
