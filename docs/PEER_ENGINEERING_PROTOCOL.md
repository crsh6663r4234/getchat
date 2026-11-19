# Codex + Claude Code ortak çalışma protokolü

## Akış

1. **Öneri:** Bir ortak hedefi ve en küçük güvenli çözümü yazar.
2. **Karşı görüş:** Diğeri varsayımları kırmaya, uç durumları ve kullanıcı
   etkisini bulmaya çalışır.
3. **Uzlaşı:** Karar, reddedilen seçenek ve test planı kayda geçer.
4. **Uygulama:** Değişiklik ayrı dalda yapılır.
5. **İnceleme:** Uygulamayan ortak diff ve test kanıtını inceler.
6. **Yayımlama:** Ancak iki görüş de kayda geçtiğinde sürüm akışına girer.

## Karar kaydı

### 2026-11-16 — Eşit ortak çalışma modeli

- Hedef: GhostChat'i Codex ve Claude Code arasında izlenebilir şekilde
  geliştirmek.
- Codex görüşü: Ortak görev kuyruğu, ayrı çalışma alanı ve test kanıtı
  kullanıcıya güvenli ilerleme sağlar.
- Claude görüşü: İlk canlı inceleme turunda eklenecek; protokol ancak `main`
  dalına alındığında tüm yeni oturumlarca görünür olur.
- Uzlaşılan karar: Kural dosyaları bu dalda hazırlanır, mevcut `main`
  çalışma alanındaki kaydedilmemiş değişiklikler korunur.
- Kilitlenme: Son ürün kararı kullanıcıdadır. Kullanıcı çevrimdışıysa güvenli
  ve geri alınabilir çözüm dışında yayımlama yapılmaz.
- Kanıt: Dosya doğrulaması, `git diff --check` ve karşılıklı inceleme.
