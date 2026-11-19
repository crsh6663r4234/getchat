# GhostChat 4.0.63: iki bilgisayarda test listesi

Hazırlık: iki Windows bilgisayarda GhostChat 4.0.63 kurulu olsun. Sunucu bir tünel arkasında ve sunucu 4.0.63 olsun. Her madde için "Geçti" ya da "Kaldı" ve kaldıysa ne gördüğünüzü (hata metni, ne yaptığınız, kaç saniye beklediğiniz) yazın.

## 1. Güncelleme ve açılış
- [ ] Uygulama açılışta güncelleme arıyor ve kendini güncelleyip yeniden açılıyor (eski bir sürümden test edin).
- [ ] Güncellemeden sonra oturum açık kalıyor, eski sohbetler görünüyor.

## 2. Birebir arama
- [ ] A, B'yi arıyor; B açtığında 10 saniye içinde bağlanıyor ve "bağlanıyor"da takılmıyor.
- [ ] İki taraf birbirini duyuyor.
- [ ] Sessize alma çalışıyor, karşı taraf sessizi duymuyor.
- [ ] Arama 5 dakika açık kaldığında kopmuyor.
- [ ] Bir taraf aramayı bitirince diğer taraf da bitiyor.
- [ ] Bir taraf uygulamayı kapatınca diğer taraf yaklaşık 30 saniyede aramayı bitiriyor.

## 3. Grup sesi
- [ ] Grup sesli odasına üç kişi katılıyor; her biri kendi adıyla ve avatarıyla görünüyor (tek kullanıcı gibi görünmemeli).
- [ ] Sonradan katılan kişi listede doğru adla görünüyor.
- [ ] Mikrofon açıkken "Mikrofonun açık" yazıyor; kapalıyken "Mikrofonun kapalı" yazıyor.
- [ ] Alt-tab yapınca kendi sesim kesilmiyor, mikrofon durumu değişmiyor.
- [ ] Birinin sesi diğerlerinden ayrı çalıyor ve ayrı kısılabiliyor.

## 4. Bas-konuş
- [ ] Ayarlar → Ses'te bas-konuş açıkken Boşluk tuşu ile konuşulabiliyor.
- [ ] Bir düğmeye tıkladıktan sonra Boşluk'a basınca düğmeye basılmış gibi olmuyor, konuşma başlıyor.
- [ ] Ayarlardan başka bir tuş (ör. V) atanabiliyor ve o tuşla konuşulabiliyor.
- [ ] Yazı yazarken tuş normal harf olarak çalışıyor, konuşmayı başlatmıyor.
- [ ] Türkçe klavyede ş, ı, ğ gibi harflerle tuş atanabiliyor; adı doğru görünüyor. (Şu an yanlış görünebilir, bunu özellikle kontrol edin.)

## 5. Doğrudan P2P (isteğe bağlı)
- [ ] Varsayılanda aramalar sunucu üzerinden geçiyor.
- [ ] P2P kutusu açılınca yeni aramalar doğrudan bağlanıyor; kapatılınca tekrar sunucu üzerinden geçiyor.

## 6. Sunucu
- [ ] `/health` adresi sürümü doğru gösteriyor (4.0.63).
- [ ] Sunucu bir sonraki yeni sürümde kendini güncelliyor (bu sürüm daha yeni bir sürüm çıkınca test edilir).

Sonuçları bana yazın; kaldıysa her madde için ne gördüğünüzü de ekleyin. Düzeltmeleri ona göre yapacağım.
