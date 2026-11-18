# İletişim sırasında gizlilik durumu — tasarım teslimi

Karar A kapsamında mevcut GhostChat ekranlarına uygulanacak tasarımdır. Yeni bir şifreleme yöntemi veya arama davranışı önermez. Türkçe ve İngilizce metinler ürün kararındaki metinlerdir.

## Sohbet ekranı

`chatHeader` içindeki mevcut `cryptoConversationStatus`, birebir veya grup sohbeti seçildiğinde sohbet başlığının altında sürekli görünür. İsim ve araç düğmelerinden sonra ayrı bir durum satırı olarak yer alır; gerekirse başlığın tüm genişliğine taşınır. Durum, mesaj listesinin kaydırılmasından etkilenmez. Hiçbir açıklama yalnızca üzerine gelince veya bir düğmeye basınca gösterilmez.

```text
  [avatar]  Deniz                                      [Ara] [Şifreleme…]
            Sohbet şifrelemesi kapalı. Mesajlar sunucuda
            düz metin olarak saklanır.
  ────────────────────────────────────────────────────────────────────
  Mesajlar …
```

Durum alanı metinle anlaşılır; isteğe bağlı simge dekoratiftir. Mevcut tema ve tipografi kullanılır; normal metinle en az 4,5:1 kontrast, okunabilir satır yüksekliği ve yeterli iç boşluk sağlanır. Uyarı tonu renk dışında metinle de anlaşılır. Açık durumu yeşil kalkan, onay işareti veya “güvenli” etiketiyle güvenlik güvencesi gibi sunmayın. Metin satır kaydırır, üç noktayla kesilmez; dar pencerede araçlar alt satıra geçebilir. Mesaj oluşturma ve gezinme kontrolleri erişilebilir kalır.

| Doğrulanmış durum | Türkçe | English |
| --- | --- | --- |
| Kapalı | Sohbet şifrelemesi kapalı. Mesajlar sunucuda düz metin olarak saklanır. | Chat encryption is off. Messages are stored as plaintext on the server. |
| Açık | Sohbet şifrelemesi açık — deneysel MLS. Yalnızca etkinleştirmeden sonra gönderilen mesajlar ve dosyalar şifrelenir. Hassas görüşmeler için güvenmeyin. | Chat encryption is on — experimental MLS. Only messages and files sent after activation are encrypted. Do not rely on it for sensitive conversations. |
| Kurulum veya doğrulama eksik | Şifreleme kurulumu veya kimlik doğrulaması tamamlanmadı. | Encryption setup or identity verification is incomplete. |
| Kontrol sürüyor | Şifreleme durumu kontrol ediliyor. | Checking encryption status. |
| Hata, eksik veya çelişkili sonuç | Şifreleme durumu doğrulanamadı. | Encryption status could not be verified. |

Kurulum ve kimlik doğrulaması için mevcut cihaza hazırlama veya kod karşılaştırma açıklaması durum metninin ardından korunur. Düğme metni ve erişilebilir adı aynı duruma uygun olmalı: kontrol, hata ve eksik kurulumda “Şifreli / Encrypted” kullanılmaz; mevcut mümkün eylem varsa eylemi adlandırır. Etkin durum düğmesi ve açıklaması birbiriyle çelişmez.

Sohbet seçimi veya yeni kontrol başladığında eski durum hemen “kontrol ediliyor” ile değiştirilir. Mimari kararındaki konuşma ve kontrol kimliği koruması uygulanır; gecikmiş sonuç başka sohbetin ya da daha yeni kontrolün göstergesini değiştirmez. Kapalı ve açık metinleri yalnızca mimarın tarif ettiği ilgili kanıtlar doğrulandığında gösterilir. `aria-live="polite"` durum değişimini duyurur; aynı metnin gereksiz tekrar yazılmasıyla tekrarlı anons üretilmez. Durum değişimi klavye odağını taşımaz.

## Arama ekranı

Mevcut `voiceStage` başlığında, bağlantı kalitesi ve bağlantı durumundan ayrı bir satırda `voice-encryption-note` kullanılır. Birebir ve grup aramalarında, gelen arama dahil ekran görünür olduğu tüm bağlantı durumlarında aynı metin gösterilir:

```text
  Deniz                                      Bağlanıyor
  Bu arama uçtan uca şifreli değil.
  [Mikrofon]  [Ekran paylaş]                         [Aramayı bitir]
```

English: **This call is not end-to-end encrypted.** Metin, bağlantı kalitesi değişirken değiştirilmez veya gizlenmez. Kompakt görünümde de tam metin görünür; ekran paylaşımı dahil hiçbir görünümde kesilmez. Kullanıcı başka bir sohbete geçtiğinde görünen `activeCallDock` içinde kişi ve bağlantı durumunun yanında aynı uyarı ayrıca gösterilir. Dar alanda eylemler ve uyarı satır kaydırır; uyarı simgeye veya araç ipucuna indirgenmez. Uyarı kapatılabilir değildir ve çağrı sona erdiğinde ilgili arama yüzeyiyle birlikte kaybolur.

Arama uyarısı normal okunabilir metindir; simge varsa `aria-hidden` olur. Bağlantı durumu için mevcut canlı bölge korunur; sabit uyarıyı her bağlantı güncellemesinde yeniden duyurmayın. Arama yüzeyleri arasında geçerken uyarı erişilebilirlik ağacında ilgili yüzeyin içinde bulunmalıdır.

## Geliştirme ve kaliteye kontrol listesi

- Her iki dilde birebir ve grup sohbetinde beş durumun tam metni; eksik kurulum için ilgili mevcut yönlendirme.
- Sohbet değiştirme, aynı sohbetin ters sırada biten kontrolleri ve başarılı kontrolden hata/kontrole geçişte eski durum ve düğme etiketinin kalmaması.
- Gelen, aranıyor, bağlanıyor, bağlı, yeniden bağlanıyor ve başarısızlık/sonlanma geçişlerinde arama uyarısı; kompakt görünüm, ekran paylaşımı ve başka sohbetteki aktif arama çubuğu.
- Dar pencere ve büyütülmüş metinde kırpılma veya yatay taşma olmaması; mevcut açık/koyu temalarda kontrast; yalnız renk veya simge olmadan anlaşılma.
- Klavye odağının korunması, sohbet durumu değişikliklerinin ekran okuyucuyla duyurulması ve sabit arama uyarısının tekrar tekrar anons edilmemesi.

Bu belge tasarım teslimidir; uygulamanın veya testlerin tamamlandığı anlamına gelmez. Kurtarma anahtarının sohbet anahtarlarını geri getirmediği uyarısının diğer kurtarma ekranlarındaki kapsamı ayrı takip konusudur.
