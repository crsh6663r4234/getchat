# GhostChat ortak görev kuyruğu

| Durum | Görev | Kabul ölçütü | Kanıt |
| --- | --- | --- | --- |
| Hazır | Mevcut yerel değişiklikleri güvenle değerlendirme | Hiçbir kaydedilmemiş dosya değiştirilmez veya kaybolmaz | Dal/diff incelemesi |
| Hazır | Sonraki kullanıcı hatasını iki taraflı ele alma | Kök neden, düzeltme ve gerileme testi yazılır | Test + karşı inceleme |
| Sürekli | Sürüm ve güncelleme akışını denetleme | Yalnızca sürümü artmış, imzalı paket yayımlanır | CI + updater feed |

## Durumlar

`Hazır` → `Tartışılıyor` → `Uygulanıyor` → `İncelemede` → `Yayımlandı`
