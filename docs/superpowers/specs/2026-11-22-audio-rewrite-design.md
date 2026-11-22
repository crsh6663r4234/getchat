# Ses sistemi: sıfırdan yeniden yazım (taslak)

Tarih: 2026-11-22. Durum: taslak, onay bekliyor. Dal: `rewrite/from-scratch`. Mesajlaşma, planlama, güncelleme ve diğer her şey olduğu gibi kalır.

## Kapsam

Yeniden yazılacak: birebir arama, grup sesi (sesli odalar), mikrofon yakalama ve gürültü temizleme, bas-konuş, cihaz seçimi ve değişimi, ses seviyesi ve kişi bazlı kısma, sesin sunucu üzerinden aktarımı.

Dokunulmayacak: mesajlaşma, şifreleme, hesap, dosya, planlama, güncelleme, sunucunun diğer uçları.

Bugünkü ses kodu şu dosyalarda: `voice-client.js`, `group-voice-client.js`, `server-media-client.js`, `audio-engine.js`, `audio-worklet.js`, ve `main.js` içindeki ses bölümleri. Yeni kod tek bir ses modülünde toplanır, eski kod silinir.

## Hedef

- Birebir ve grup sesi, iki bilgisayarda 10 saniyeden kısa sürede bağlanır.
- Sesli odada kimin konuştuğu, kimin mikrofonunun kapalı olduğu bir bakışta anlaşılır.
- Bas-konuş, cihaz değişimi, alt-tab, uygulamayı yeniden açma gibi durumlarda ses kesilmez ya da dürüstçe "bağlantı yeniden kuruluyor" der.
- Her ses yolunun (aktarım modu) testi otomatik çalışır.

## Tasarım

- **Tek ses modülü:** Durum makinesi: boşta → bağlanıyor → bağlı → yeniden bağlanıyor → kapalı. Arayüz yalnızca bu durumu okur, ses kodunu bilmez.
- **Tek aktarım yolu:** Sunucu üzerinden ses (WebSocket, mevcut relay). Doğrudan bağlantı (P2P) isteğe bağlı kalır. LiveKit bu sürümde yok.
- **Ses işleme:** Gürültü temizleme için tek motor (RNNoise). DeepFilterNet kaldırılır, çünkü ikisini birden sürdürmek ve test etmek gereksiz yük.
- **Zaman ve gecikme:** Her ses paketinin sıra numarası ve zaman damgası var; kayıp ve gecikme ölçülüp arayüze gösterilebilir.

## Önerilen varsayılanlar (onayınızı istiyorum)

1. Aktarım: sadece sunucu relay, P2P isteğe bağlı.
2. Gürültü temizleme: sadece RNNoise, DeepFilterNet kaldırılır.
3. Grup sesi: LiveKit kaldırılır, sunucu relay tek yol olur.
4. Eski ses kodu silinir (ayrı bir dalda yeniden üretilebilir).

## Aşamalar

1. Ses modülü ve durum makinesi, Node testleri.
2. Sunucu relay tarafı: tek oda, üyelik kontrolleri, kare sınırları (sunucu kodu bu kapsamda, ama mesajlaşma dokunulmaz).
3. Birebir arama, sonra grup sesi.
4. Bas-konuş, cihaz değişimi, alt-tab ve yeniden bağlanma.
5. İki bilgisayarda deneme, sonra yayın.

## Açık nokta

Sunucu tarafındaki ses relay'i yeniden yazmak mı, yoksa mevcut relay'i olduğu gibi kullanıp yalnızca istemciyi yazmak mı? Önerim: istemci tam yeni, sunucu relay'i mevcut kalır ve gerekirse sonra değişir.
