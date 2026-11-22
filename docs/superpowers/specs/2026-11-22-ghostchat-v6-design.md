# GhostChat v6: sıfırdan tasarım (taslak)

Tarih: 2026-11-22. Durum: taslak, onay bekliyor. Dal: `rewrite/from-scratch` (main ve 5.0.0 dokunulmadı).

## Amaç

Üç şeyi aynı uygulamada, bir bütün olarak sunmak:
1. **Mesajlaşma:** birebir ve grup sohbeti, dosya paylaşımı, arama.
2. **Sesli odalar:** Discord benzeri kalıcı oda ve birebir arama; konuşan kişi, mikrofon durumu, bas-konuş.
3. **Ekip özellikleri:** planlama (anket, etkinlik), grup yönetimi, izleme partisi.

Başarı ölçütü: iki bilgisayarda birebir ve grup sesi 10 saniyeden kısa sürede bağlanıyor, sesli odada kimin konuştuğu anlaşılıyor, uygulama güncellemeyi kendiliğinden kuruyor.

## Kapsam dışı (ilk sürüm)

Ekran paylaşımı kalitesi (sonraya), uçtan uca şifrelemeli sesli oda (sonraya), mobil uygulama.

## Yaklaşım seçenekleri

- **A. Sadece arayüz:** Mevcut Rust sunucusu ve masaüstü çekirdeği kalır, arayüz modüler bir yapıya (TypeScript, bileşenlere bölünmüş) yeniden yazılır. Riski en düşük, süresi kısa.
- **B. Sunucu ve arayüz birlikte:** Sunucu API'si sadeleştirilir, sesli oda baştan tasarlanır. Daha temiz, ama daha uzun ve test gerektirir.
- **C. Tam sıfır:** Tauri de dahil her şey yeniden. En büyük iş, en çok risk.

**Önerim: B.** A yalnızca arayüzü temizler ama sesli odanın üç farklı yolu (doğrudan, sunucu relay, LiveKit) yerinde kalır; sorunlarımızın büyük kısmı oradan geliyor. B'de sesli oda tek bir yola indirilir.

## Mimari (B için)

- **Sunucu:** Rust, Axum, SQLite. Modüller: kimlik, sohbet, oda, dosya, planlama, güncelleme. Sesli oda için tek bir ses yolu (sunucu relay), LiveKit yalnızca opsiyonel olarak sonra eklenir.
- **İstemci:** Tauri 2, arayüz TypeScript, bileşenlere bölünmüş. Ses motoru ayrı bir modül, UI'dan bağımsız test edilir.
- **Protokol:** sürüm numaralı; eski istemciye açık "güncelleyin" mesajı.
- **Veri:** sunucu şeması baştan; eski veritabanından geçiş betiği yazılır, eski kullanıcılar hesaplarını kaybetmez.

## Aşamalar

1. Sunucu çekirdeği: hesap, oturum, birebir sohbet, dosya. Testler.
2. İstemci iskeleti: giriş, sohbet ekranı, mesajlaşma. İki bilgisayarda deneme.
3. Sesli oda: tek ses yolu, konuşan göstergesi, bas-konuş. İki bilgisayarda deneme.
4. Grup, planlama, izleme partisi.
5. Güncelleme, sürüm uyumluluğu, geçiş betiği, yayın.

Her aşama sonunda çalışır bir sürüm ve iki bilgisayar testi olmadan bir sonraki aşamaya geçilmez.

## Açık sorular (sizin kararınız)

1. Yaklaşım A, B veya C? (Önerim B.)
2. Mevcut kullanıcılar ve sunucu geçişi: eski veritabanı taşınsın mı, yoksa yeni sürüm temiz bir başlangıç mı olsun?
3. Şifreleme v6'da kalsın mı? (Şimdilik tasarımda yer almıyor; istenirse aşama 4 sonrası eklenir.)
