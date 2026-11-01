(() => {
  const DEFAULT_REMOTE_SERVER = "https://ghost-cassper.duckdns.org";
  const ENCRYPTED_ATTACHMENT_MARKER = "ghostchat-file-v1:";
  const LEGACY_DEFAULT_REMOTE_SERVER = "https://185.158.151.179";
  const LOCAL_SERVER_SENTINEL = "__ghostchat_local__";
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const invoke = window.__TAURI__?.core?.invoke;
  const state = { localBase: "", base: "", token: "", user: null, presence: new Map(), presenceMode: "online", pendingRecovery: "", recoveryKey: "", pendingGroupInviteCode: null, page: "messages", conversations: [], friends: [], pending: [], blockedUserIds: new Set(), attachmentObserver: null, attachmentObjectUrls: [], seenFriendRequestIds: new Set(), active: null, older: true, reconnect: 1200, socket: null, searchTab: "users", updateVersion: null, updateNotifiedVersion: null, cryptoDevice: null, cryptoEnabled: new Set(), cryptoLocked: new Set(), cryptoSetupComplete: new Set(), cryptoCanManage: new Set(), cryptoPeerFingerprints: new Map(), cryptoSyncing: new Map(), cryptoMessageDecoding: new Map(), cryptoMemberIds: new Map(), cryptoMembershipPending: new Set(), cryptoMembershipOperations: new Map(), replyingTo: null, lastReadSequence: new Map(), typingUsers: new Map(), typingTimers: new Map(), typingActive: false, typingStopTimer: null, typingSentAt: 0, groupInviteCodes: new Map(), groupVoiceAvailable: null };
  function resetCryptoViewState() {
    state.cryptoEnabled.clear(); state.cryptoLocked.clear(); state.cryptoSetupComplete.clear(); state.cryptoCanManage.clear(); state.cryptoPeerFingerprints.clear(); state.cryptoMembershipPending.clear();
    state.cryptoSyncing.clear(); state.cryptoMessageDecoding.clear(); state.cryptoMemberIds.clear(); state.cryptoMembershipOperations.clear();
  }
  let signupAvatarData = null;
  let profileAvatarData = null;
  let settingsRecoveryStatusKey = "";
  const trText = {
    "Create account": "Hesap oluştur", "Recover": "Kurtar", "Name": "Ad", "Display name": "Görünen ad", "Profile picture": "Profil fotoğrafı", "Choose": "Seç", "Create anonymous account": "Anonim hesap oluştur", "Recover account": "Hesabı kurtar", "Recovery key": "Kurtarma anahtarı", "Your recovery key": "Kurtarma anahtarın", "Copy key": "Anahtarı kopyala", "Save backup": "Yedek kaydet", "Continue to GhostChat": "GhostChat'e devam et", "Messages": "Mesajlar", "Friends": "Arkadaşlar", "Find people": "Kişi bul", "Search messages": "Mesajlarda ara", "Settings": "Ayarlar", "Profile": "Profil", "Connection": "Bağlantı", "Appearance": "Görünüm", "Notifications": "Bildirimler", "Voice and screen sharing": "Sesli sohbet ve ekran paylaşımı", "Updates": "Güncellemeler", "Privacy and data": "Gizlilik ve veriler", "Theme": "Tema", "Text size": "Yazı boyutu", "Dark": "Koyu", "Light": "Açık", "Compact": "Küçük", "Default": "Varsayılan", "Large": "Büyük", "Windows notifications": "Windows bildirimleri", "Send test notification": "Test bildirimi gönder", "Microphone": "Mikrofon", "Speaker": "Hoparlör", "System default": "Sistem varsayılanı", "Refresh audio devices": "Ses aygıtlarını yenile", "Echo cancellation": "Yankı engelleme", "Noise suppression": "Gürültü azaltma", "Automatic microphone gain": "Otomatik mikrofon seviyesi", "Push to talk (hold Space)": "Bas-konuş (Boşluk tuşunu basılı tut)", "Share system audio with screen": "Ekranla sistem sesini paylaş", "Save settings": "Ayarları kaydet", "Check now": "Şimdi kontrol et", "Install and restart": "Yükle ve yeniden başlat", "Save profile": "Profili kaydet", "Remove photo": "Fotoğrafı kaldır", "Your quiet corner": "Sana özel alan", "A little more room to talk.": "Rahatça konuşabileceğin bir alan.", "Find someone by username, send a friend request, then start a private conversation. Your account does not need an email or phone number.": "Etiketiyle kişi bul, arkadaşlık isteği gönder ve özel sohbete başla. Hesap için e-posta veya telefon gerekmez.", "View friends": "Arkadaşları gör", "Search": "Ara", "Search by username or user ID. GhostChat does not publish a public directory.": "Kullanıcı etiketiyle veya kullanıcı kimliğiyle ara. GhostChat herkese açık bir rehber yayınlamaz.", "No conversations yet. Find a friend to start a chat.": "Henüz sohbet yok. Sohbet başlatmak için arkadaş ekle.", "No pending requests.": "Bekleyen istek yok.", "YOUR FRIENDS": "ARKADAŞLARIN", "REQUESTS": "İSTEKLER", "DIRECT MESSAGES": "ÖZEL MESAJLAR", "No friends yet. Use Find people to add someone.": "Henüz arkadaşın yok. Kişi bul bölümünden arkadaş ekleyebilirsin.", "Manage requests and start a private conversation.": "İstekleri yönet ve özel sohbet başlat.", "All friends": "Tüm arkadaşlar", "Accept": "Kabul et", "Ignore": "Yoksay", "Message": "Mesaj", "Add friend": "Arkadaş ekle", "Request sent": "İstek gönderildi", "No friend requests right now.": "Şu anda arkadaşlık isteği yok.", "No matching messages.": "Eşleşen mesaj yok.", "Edited": "Düzenlendi", "Edit": "Düzenle", "Delete": "Sil", "Write a message…": "Mesaj yaz…", "Load earlier messages": "Önceki mesajları yükle", "Call": "Ara", "End call": "Aramayı bitir", "Settings saved.": "Ayarlar kaydedildi.", "Profile saved.": "Profil kaydedildi.", "Friend request accepted.": "Arkadaşlık isteği kabul edildi.", "Request removed.": "İstek kaldırıldı.", "Choose an image file.": "Bir resim dosyası seç.", "That photo is too detailed. Choose a smaller image.": "Fotoğraf çok detaylı. Daha küçük bir görsel seç.", "Could not use that photo.": "Fotoğraf kullanılamadı.", "Checking for updates": "Güncellemeler kontrol ediliyor", "Update required": "Güncelleme gerekli", "Downloading update": "Güncelleme indiriliyor", "Update failed": "Güncelleme başarısız", "Try again": "Yeniden dene", "Getting the latest version ready…": "En yeni sürüm hazırlanıyor…", "This app version is out of date.": "Bu uygulama sürümü güncel değil.", "Preparing the update…": "Güncelleme hazırlanıyor…"
  };
  Object.assign(trText, {
    "is downloading": "indiriliyor",
    "Choose a friend or find someone new.": "Bir arkadaş seç ya da yeni birini bul.", "Connected": "Bağlandı", "Reconnecting": "Yeniden bağlanıyor", "Voice ready": "Ses hazır", "Incoming call": "Gelen arama", "Calling…": "Aranıyor…", "Connecting…": "Bağlanıyor…", "In voice": "Sesli görüşmede", "Reconnecting…": "Yeniden bağlanıyor…", "Voice": "Ses", "Voice call": "Sesli arama", "Try microphone": "Mikrofonu dene", "Mic off": "Mikrofon kapalı", "Speaking": "Konuşuyor", "Hold Space": "Boşluk tuşunu basılı tut", "Turn mic on": "Mikrofonu aç", "Mute mic": "Mikrofonu kapat", "Sound off": "Ses kapalı", "Sound on": "Ses açık", "Starting…": "Başlatılıyor…", "Stop sharing": "Paylaşımı durdur", "Share screen": "Ekranı paylaş", "No microphone detected. Calls can still be joined muted so you can listen.": "Mikrofon bulunamadı. Yine de sessiz katılıp dinleyebilirsin.", "Microphone access is unavailable. You can stay muted or allow access and try again.": "Mikrofon erişimi yok. Sessiz kalabilir veya izin verip tekrar deneyebilirsin.", "No microphone found. You can still join calls muted.": "Mikrofon bulunamadı. Aramalara sessiz katılabilirsin.", "Audio devices refreshed.": "Ses aygıtları yenilendi.", "Checking voice relay support…": "Ses aktarım desteği kontrol ediliyor…", "Server address": "Sunucu adresi", "Selected server": "Seçili sunucu", "Local server": "Yerel sunucu", "Search @username or user ID": "@etiket veya kullanıcı kimliği ara", "Search people": "Kişi ara", "Search in your messages": "Mesajlarında ara", "Search for a username or user ID. Users are not listed publicly.": "Kullanıcı etiketiyle veya kimliğiyle ara. Kullanıcılar herkese açık listelenmez.", "Searches the 50 latest messages in each conversation on this device; your search phrase stays local.": "Bu cihazda her sohbetin son 50 mesajını arar; arama ifaden cihazında kalır.", "Searching locally…": "Yerel olarak aranıyor…", "Search could not load your messages.": "Mesajların yüklenemedi.", "No users found. Check the spelling and try again.": "Kullanıcı bulunamadı. Yazımı kontrol edip tekrar dene.", "Searching…": "Aranıyor…", "You are logged out.": "Oturum kapatıldı.", "Preferences exported.": "Tercihler dışa aktarıldı.", "Account data exported.": "Hesap verileri indirildi.", "Call audio is still blocked by Windows or the audio device.": "Arama sesi Windows veya ses aygıtı tarafından engelleniyor."
  });
  Object.assign(trText, {
    "Username": "Kullanıcı adı", "3–24 letters, numbers, or underscores. Your tag changes when you save.": "3–24 harf, rakam veya alt çizgi. Kaydedince etiketin değişir.", "That username is already in use.": "Bu kullanıcı adı zaten kullanılıyor.", "Username must be 3 to 24 letters, numbers, or underscores.": "Kullanıcı adı 3–24 harf, rakam veya alt çizgi içermeli.", "Display name must be 2 to 32 characters.": "Görünen ad 2–32 karakter olmalı.", "Recovery key": "Kurtarma anahtarı", "Show recovery key": "Kurtarma anahtarını göster", "Hide recovery key": "Kurtarma anahtarını gizle", "Copy key": "Anahtarı kopyala", "Save backup": "Yedek kaydet", "Generate replacement key": "Yeni kurtarma anahtarı oluştur", "Creating a replacement immediately invalidates your previous recovery key. Keep the new key somewhere safe; anyone who has it can recover your account.": "Yeni anahtar oluşturmak eski kurtarma anahtarını hemen geçersiz kılar. Yeni anahtarı güvenli yerde sakla; anahtara sahip olan herkes hesabını kurtarabilir.", "Recovery key saved securely on this device.": "Kurtarma anahtarı bu cihazda güvenli biçimde saklanıyor.", "This device has no saved key. Creating a replacement will disable your old key.": "Bu cihazda kayıtlı anahtar yok. Yeni anahtar oluşturmak eski anahtarını devre dışı bırakır.", "Recovery key copied.": "Kurtarma anahtarı kopyalandı.", "Create a replacement recovery key? Your old key will stop working immediately.": "Yeni kurtarma anahtarı oluşturulsun mu? Eski anahtarın hemen geçersiz olacak.", "A new recovery key is ready. Save a backup somewhere safe.": "Yeni kurtarma anahtarın hazır. Güvenli bir yere yedekle.", "Windows could not save this key. Save the displayed backup before closing Settings.": "Windows bu anahtarı kaydedemedi. Ayarları kapatmadan önce görünen anahtarı yedekle.", "Windows could not store a secure copy. Copy or save the key before continuing.": "Windows anahtarın güvenli bir kopyasını saklayamadı. Devam etmeden önce anahtarı kopyala veya kaydet.", "Recovery key changes are temporarily limited. Please try again later.": "Kurtarma anahtarı değişiklikleri geçici olarak sınırlandı. Daha sonra tekrar dene."
  });
  Object.assign(trText, {
    "Start a conversation": "Sohbet başlat", "Create group": "Grup oluştur", "Create a group": "Grup oluştur", "Group name": "Grup adı", "CHOOSE 2 TO 15 FRIENDS": "2 İLE 15 ARKADAŞ SEÇ", "Only accepted friends can be added. You can invite up to 15 friends.": "Yalnızca arkadaş olarak eklediğin kişileri seçebilirsin. En fazla 15 arkadaş davet edebilirsin.", "Start a private chat or bring a few friends together.": "Özel sohbet başlat ya da birkaç arkadaşını bir araya getir.", "CONVERSATIONS": "SOHBETLER", "GROUP MEMBERS": "GRUP ÜYELERİ", "No friends to add yet. Add friends first.": "Henüz eklenecek arkadaş yok. Önce arkadaş ekle.", "Choose 2 to 15 friends for a group.": "Grup için 2 ile 15 arkadaş seç.", "Group created.": "Grup oluşturuldu.", "members": "üye", "You were added to a group.": "Bir gruba eklendin.", "Attach a photo, video, or file": "Fotoğraf, video veya dosya ekle", "File is too large. The limit is 5 GB.": "Dosya çok büyük. Üst sınır 5 GB.", "File sent.": "Dosya gönderildi.", "Could not load this file.": "Bu dosya yüklenemedi.",
    "Microphone test": "Mikrofon testi", "Press test and speak into your microphone.": "Testi başlatıp mikrofona konuş.", "Speak into your microphone.": "Mikrofona konuş.", "Microphone is picking up sound.": "Mikrofon ses alıyor.", "Test microphone": "Mikrofonu dene", "Stop microphone test": "Mikrofon testini durdur", "Microphone test stopped.": "Mikrofon testi durduruldu.",
    "Refresh share": "Paylaşımı yenile", "File shared": "Dosya paylaşıldı",
    "You": "Sen", "Who’s here": "Görüşmedekiler", "Listening": "Dinliyor", "Microphone on": "Mikrofon açık", "Mic off": "Mikrofon kapalı", "No microphone": "Mikrofon yok", "Muted for you": "Sana sessiz", "Mute their audio": "Onun sesini kıs", "Unmute their audio": "Onun sesini aç", "Only changes what you hear.": "Yalnızca senin duyduğun sesi etkiler.", "Right-click to mute or unmute this caller": "Arayanın sesini kısmak/açmak için sağ tıkla", "DIRECT MESSAGE": "ÖZEL MESAJ", "GROUP CONVERSATION": "GRUP SOHBETİ"
  });
  Object.assign(trText, {
    "Preparing this device…": "Bu cihaz hazırlanıyor…", "Encryption device is ready; one key package was published.": "Şifreleme cihazı hazır; bir anahtar paketi yayımlandı.", "The device identity was prepared, but its key package could not be published.": "Cihaz kimliği hazırlandı ancak anahtar paketi yayımlanamadı.", "A local device identity is saved. Publish a fresh one-time key package when needed.": "Yerel cihaz kimliği kayıtlı. Gerektiğinde yeni tek kullanımlık anahtar paketi yayımla.", "No encryption device is prepared on this account yet.": "Bu hesap için henüz şifreleme cihazı hazırlanmadı.", "Device ID": "Cihaz kimliği", "Identity fingerprint": "Kimlik parmak izi", "Private messaging setup": "Özel mesaj hazırlığı", "Prepare this device": "Bu cihazı hazırla", "Prepare this device and publish a one-time key package. This only prepares the device; direct-message encryption starts after both people set it up and verify each other.": "Bu cihazı hazırla ve tek kullanımlık anahtar paketi yayımla. Bu yalnızca cihazı hazırlar; özel mesaj şifrelemesi iki taraf da kurulumu tamamlayıp birbirini doğrulayınca başlar.", "Encrypted chat keys are stored on this Windows device. Your account recovery key does not restore them. Reinstalling Windows or losing this device can make existing encrypted chats unreadable; device backup and transfer are not supported yet.": "Şifreli sohbet anahtarları bu Windows cihazında tutulur. Hesap kurtarma anahtarın bunları geri yüklemez. Windows'u yeniden kurmak veya bu cihazı kaybetmek mevcut şifreli sohbetleri okunamaz hâle getirebilir; cihaz yedekleme ve aktarımı henüz desteklenmiyor."
  });
  Object.assign(trText, {
    "Manage group": "Grubu yönet", "GROUP SETTINGS": "GRUP AYARLARI", "Members": "Üyeler", "Invite friends": "Arkadaş davet et", "Add selected friends": "Seçilen arkadaşları ekle", "Save name": "Adı kaydet", "Owner": "Kurucu", "Admin": "Yönetici", "Member": "Üye", "Leave group": "Gruptan ayrıl", "Remove from group": "Gruptan çıkar", "No friends available to invite.": "Davet edilecek arkadaş kalmadı.", "Group name saved.": "Grup adı kaydedildi.", "Friends added to the group.": "Arkadaşlar gruba eklendi.", "Group member updated.": "Grup üyesi güncellendi.", "You left the group.": "Gruptan ayrıldın.", "Member removed from the group.": "Üye gruptan çıkarıldı.", "Transfer group ownership to this person? You will become an admin.": "Grup sahipliğini bu kişiye devredilsin mi? Sen yönetici olacaksın.", "Group settings could not be loaded.": "Grup ayarları yüklenemedi.", "Could not update the group.": "Grup güncellenemedi.", "Loading group settings…": "Grup ayarları yükleniyor…", "Adding friends…": "Arkadaşlar ekleniyor…", "Groups can have up to 16 members.": "Gruplar en fazla 16 üyeli olabilir.", "Transfer group ownership before leaving the group.": "Gruptan ayrılmadan önce grup sahipliğini devret.", "Reply": "Yanıtla", "Replying to": "Yanıtlanan", "Cancel reply": "Yanıtı iptal et", "Message reactions": "Mesaj tepkileri", "Add a reaction": "Tepki ekle", "Pinned": "Sabitlendi", "Pin message": "Mesajı sabitle", "Unpin message": "Sabitlemeyi kaldır", "Typing…": "Yazıyor…", "is typing…": "yazıyor…", "and": "ve", "others": "kişi daha", "Unread messages": "Okunmamış mesaj", "Seen": "Görüldü"
  });
  Object.assign(trText, {
    "Join voice": "Sesliye katıl", "Join group voice": "Grup seslisine katıl", "Group call": "Grup araması", "Group voice ready": "Grup sesi hazır", "Group voice connected": "Grup sesine bağlandı", "Group voice not configured": "Grup sesi yapılandırılmadı", "Group voice is not configured on this server. Configure LiveKit in the server environment.": "Bu sunucuda grup sesi yapılandırılmamış. Sunucu ortamında LiveKit'i ayarla.", "Group voice status could not be checked.": "Grup sesi durumu kontrol edilemedi.", "Group voice status is being checked.": "Grup sesi durumu kontrol ediliyor.", "Group voice SFU is configured. Group media is relayed and not end-to-end encrypted.": "Grup sesi SFU'su yapılandırılmış. Grup medyası aktarılır ve uçtan uca şifreli değildir.", "Leave call": "Aramadan ayrıl", "RELAYED · NOT END-TO-END ENCRYPTED": "AKTARILIR · UÇTAN UCA ŞİFRELİ DEĞİL", "Group voice is relayed by a LiveKit SFU and is not end-to-end encrypted. Your microphone is off.": "Grup sesi LiveKit SFU üzerinden aktarılır ve uçtan uca şifreli değildir. Mikrofonun kapalı.", "Group voice is relayed by a LiveKit SFU and is not end-to-end encrypted. No microphone was found; you joined muted.": "Grup sesi LiveKit SFU üzerinden aktarılır ve uçtan uca şifreli değildir. Mikrofon bulunamadı; sessiz katıldın.", "Group voice is not configured on this server.": "Bu sunucuda grup sesi yapılandırılmamış.", "The group voice module did not load. Restart GhostChat and try again.": "Grup sesi yüklenemedi. GhostChat'i yeniden başlatıp tekrar dene.", "Microphone stayed off": "Mikrofon kapalı kaldı", "Screen share": "Ekran paylaşımı"
  });
  Object.assign(trText, { "Invite codes": "Davet kodları", "Each code expires after 7 days, allows up to 20 joins, and is shown only once.": "Her kod 7 gün sonra geçersiz olur, en fazla 20 katılıma izin verir ve yalnızca bir kez gösterilir.", "Create invite code": "Davet kodu oluştur", "Invite code": "Davet kodu", "Only use invite codes from people you trust.": "Yalnızca güvendiğin kişilerden gelen davet kodlarını kullan.", "Join group": "Gruba katıl", "Join with invite code": "Davet koduyla katıl", "No active invite codes.": "Etkin davet kodu yok.", "Expires": "Son kullanım", "Uses": "Kullanım", "Copy": "Kopyala", "Revoke": "İptal et", "Invite code copied.": "Davet kodu kopyalandı.", "Group invite code created.": "Grup davet kodu oluşturuldu.", "Group invite revoked.": "Grup daveti iptal edildi.", "Joined group.": "Gruba katıldın.", "Could not copy invite code.": "Davet kodu kopyalanamadı.", "Joining…": "Katılınıyor…" });
  Object.assign(trText, {
    "Encrypted groups support verified friend additions. Leaving and removing members remain unavailable until safe MLS removal is supported.": "Şifreli gruplara doğrulanmış arkadaş eklenebilir. Güvenli MLS üye çıkarma desteği gelene kadar gruptan ayrılma ve üye çıkarma kapalı kalır.",
    "Verify each selected friend's full security code through another trusted channel before adding them to this encrypted group.": "Seçtiğin her arkadaşın tam güvenlik kodunu şifreli gruba eklemeden önce başka güvenilir bir kanaldan doğrula.",
    "Pending encrypted membership update is being retried…": "Bekleyen şifreli grup güncellemesi yeniden deneniyor…",
    "The encrypted group membership was updated. Verify the new full group code with its members before sending messages.": "Şifreli grup üyeliği güncellendi. Mesaj göndermeden önce yeni tam grup kodunu üyelerle doğrula.",
    "This encrypted group can only be changed from a device that has joined its MLS group.": "Bu şifreli grup yalnızca MLS grubuna katılmış bir cihazdan değiştirilebilir.",
    "Choose accepted friends from the list.": "Listeden arkadaş olarak eklenmiş kişileri seç.",
    "Encryption update cancelled. The one-time key packages were already claimed; the selected friends may need to publish fresh packages.": "Şifreleme güncellemesi iptal edildi. Tek kullanımlık anahtar paketleri alındı; seçilen arkadaşların yenilerini yayımlaması gerekebilir.",
    "This encrypted group uses MLS for text messages. Existing messages and previous files are not encrypted; new attachments use MLS-wrapped file keys. Membership changes require verified MLS updates.": "Bu şifreli grup metin mesajlarında MLS kullanıyor. Eski mesajlar ve önceki dosyalar şifreli değil; yeni dosyalar MLS ile korunan anahtarlarla gönderilir. Üyelik değişikliği doğrulanmış MLS güncellemesi gerektirir.",
    "Verify the full group security code with every member through another trusted channel before continuing. The list below shows each member's device code.": "Devam etmeden önce grubun güvenlik kodunu her üyeyle başka, güvenilir bir kanaldan karşılaştır. Aşağıda her üyenin cihaz kodu yer alıyor.",
    "Group encryption can only be started by a group owner or admin.": "Grup şifrelemesini yalnızca grup kurucusu veya yöneticisi başlatabilir.",
    "Encrypted group membership is locked.": "Şifreli grup üyeliği kilitli.",
    "The group roster changed while encryption was being prepared. Refresh the group and try again.": "Şifreleme hazırlanırken grup üyeleri değişti. Grubu yenileyip tekrar dene.",
    "This group is locked for MLS encryption. Prepare this device in Settings to read new messages; ask a group admin to help if setup still fails.": "Bu grup MLS şifrelemesi için kilitli. Yeni mesajları okumak için Ayarlar'dan bu cihazı hazırla; kurulum olmazsa grup yöneticisinden yardım iste.",
    "This conversation is locked for MLS encryption. Prepare this device in Settings to read new messages; ask the person who started setup for help if it still fails.": "Bu sohbet MLS şifrelemesi için kilitli. Yeni mesajları okumak için Ayarlar'dan bu cihazı hazırla; sorun sürerse kurulumu başlatan kişiden yardım iste.",
    "This conversation is locked while MLS encryption is being set up. Finish setup before sending messages.": "MLS şifrelemesi hazırlanırken bu sohbet kilitlendi. Mesaj göndermeden önce kurulumu tamamla.",
    "This encrypted conversation only accepts MLS-encrypted text messages.": "Bu şifreli sohbet yalnızca MLS ile şifrelenmiş metin mesajlarını kabul eder.",
    "File sharing is disabled in this encrypted conversation until encrypted attachments are supported.": "Bu şifreli sohbette yeni dosyalar şifrelenerek gönderilir; eski dosyalar şifresiz kalır.",
    "Messages cannot be edited in an encrypted conversation; send a new encrypted message instead.": "Şifreli sohbetteki mesajlar düzenlenemez; değişikliği yeni bir şifreli mesajla gönder.",
    "Preview securely": "Güvenli önizleme",
    "Save decrypted file": "Çözülmüş dosyayı kaydet",
    "Encrypting and uploading securely…": "Güvenli biçimde şifrelenip yükleniyor…",
    "Downloading and decrypting securely…": "Güvenli biçimde indirilip çözülüyor…",
    "Secure file sent.": "Şifreli dosya gönderildi.",
    "Encrypted file saved.": "Şifreli dosya çözüldü ve kaydedildi.",
    "Encrypted attachment": "Şifreli dosya",
    "The encrypted file could not be previewed.": "Şifreli dosyanın önizlemesi açılamadı.",
    "This encrypted attachment could not be opened.": "Şifreli dosya eki açılamadı.",
    "Encrypted attachments require MLS encryption to be enabled for this conversation.": "Şifreli dosya göndermek için bu sohbette MLS şifrelemesi açık olmalı.",
    "Continue encryption setup": "Şifreleme kurulumuna devam et",
    "Invite codes are disabled for encrypted groups; use the MLS membership flow instead.": "Şifreli gruplarda davet kodları kapalıdır; üyelik için MLS akışını kullan.",
    "The encrypted group identity is not verified. Compare the full group code with its members before sending private messages.": "Şifreli grup kimliği doğrulanmadı. Özel mesaj göndermeden önce grup kodunu üyelerle karşılaştır.",
    "Encryption setup was cancelled. The one-time key packages were already claimed; members may need to publish fresh packages.": "Şifreleme kurulumu iptal edildi. Tek kullanımlık anahtar paketleri alınmış oldu; üyelerin yenilerini yayımlaması gerekebilir.",
    "This encrypted group only accepts MLS-encrypted text messages.": "Bu şifreli grup yalnızca MLS ile şifrelenmiş metin mesajlarını kabul eder.",
    "File sharing is disabled in this encrypted group until encrypted attachments are supported.": "Bu şifreli grupta yeni dosyalar şifrelenerek gönderilir; eski dosyalar şifresiz kalır.",
    "Messages cannot be edited in an encrypted group; send a new encrypted message instead.": "Şifreli gruptaki mesajlar düzenlenemez; değişikliği yeni bir şifreli mesajla gönder.",
  });
  Object.assign(trText, {
    "Enable encryption": "Şifrelemeyi etkinleştir", "Encrypted": "Şifreli", "Setting up encryption…": "Şifreleme hazırlanıyor…", "Encrypted messages are ready.": "Şifreli mesajlar hazır.", "Encrypted conversation": "Şifreli sohbet", "File sharing is not encrypted in this conversation yet.": "Şifreli sohbetlerde yeni dosyalar şifrelenerek gönderilir; eski dosyalar şifresiz kalır.", "This direct message now uses MLS encryption. Messages sent before setup remain stored as plain text on the server; file sharing is not encrypted yet.": "Bu özel mesaj artık MLS ile şifreleniyor. Kurulumdan önce gönderilen mesajlar ve önceki dosyalar sunucuda düz metin olarak kalır; yeni dosyalar şifrelenerek gönderilir.", "Prepare this device for encryption in Settings first.": "Önce Ayarlar'dan bu cihazı şifrelemeye hazırla.", "Your friend needs to prepare an encryption device first.": "Arkadaşının önce bir şifreleme cihazı hazırlaması gerekiyor.", "The one-time key was used. Ask your friend to publish a fresh encryption key package in Settings.": "Tek kullanımlık anahtar kullanıldı. Arkadaşından Ayarlar'dan yeni bir şifreleme anahtarı yayımlamasını iste.", "Compare this security code with your friend through another trusted channel before continuing. Continue only if the codes match.": "Devam etmeden önce bu güvenlik kodunu başka, güvenilir bir kanaldan arkadaşınla karşılaştır. Yalnızca kodlar eşleşirse devam et.", "The encrypted identity is not verified. Compare this code with your friend before sending private messages.": "Şifreli kimlik doğrulanmadı. Özel mesaj göndermeden önce bu kodu arkadaşınla karşılaştır.", "Encrypted message": "Şifreli mesaj", "This message could not be decrypted on this device.": "Bu mesaj bu cihazda çözülemedi.", "Encryption could not be enabled for this conversation.": "Bu sohbet için şifreleme etkinleştirilemedi.", "The encrypted message is waiting to be sent. Try again when connected.": "Şifreli mesaj gönderilmeyi bekliyor. Bağlantı gelince tekrar dene."
  });
  Object.assign(trText, {
    "The verified security code changed. Compare the old and current codes with everyone in this conversation through another trusted channel. Continue only if you expected this identity or membership change; canceling keeps new messages blocked.": "Doğrulanmış güvenlik kodu değişti. Eski ve yeni kodları bu sohbettekilerle başka, güvenilir bir kanaldan karşılaştır. Yalnızca bu kimlik veya üyelik değişikliğini bekliyorsan devam et; iptal edersen yeni mesajlar kapalı kalır.",
    "Previous verified code": "Önceden doğrulanmış kod", "Current security code": "Şu anki güvenlik kodu", "The local encrypted event cursor is invalid.": "Yerel şifreli olay imleci geçersiz."
  });
  Object.assign(trText, {
    "Leave this encrypted group? The request will be sent to the owner. You will stay in the group until the owner commits the MLS update. Owners must transfer ownership before leaving.": "Bu şifreli gruptan ayrılmak istiyor musun? İstek grup sahibine gönderilecek. Sahip MLS güncellemesini tamamlayana kadar grupta kalacaksın. Sahipler ayrılmadan önce sahipliği devretmeli.",
    "Leave request sent. You will remain in the group until its owner approves the encrypted update.": "Ayrılma isteği gönderildi. Grup sahibi şifreli güncellemeyi onaylayana kadar grupta kalacaksın.",
    "An encrypted group member requested to leave. The owner will apply the MLS update.": "Şifreli grup üyesi ayrılma isteği gönderdi. MLS güncellemesini grup sahibi uygulayacak.",
    "Encrypted groups support verified additions, removals, and owner-approved leave requests.": "Şifreli gruplarda doğrulanmış üye ekleme, çıkarma ve sahip onaylı ayrılma istekleri desteklenir.",
    "The owner must resolve other pending MLS proposals before this leave can finish.": "Ayrılma tamamlanmadan önce grup sahibi diğer bekleyen MLS isteklerini çözmeli.",
    "Transfer group ownership before requesting to leave.": "Ayrılma isteği göndermeden önce grup sahipliğini devret.",
    "Finish encrypted group setup before requesting to leave.": "Ayrılma isteği göndermeden önce şifreli grup kurulumunu tamamla.",
    "Finish encrypted group setup before changing its members.": "Üyeleri değiştirmeden önce şifreli grup kurulumunu tamamla.",
    "Another encrypted group leave request is already waiting for the owner.": "Grup sahibinin onaylamasını bekleyen başka bir şifreli ayrılma isteği var.",
    "Resolve the pending encrypted leave request before changing group members.": "Grup üyelerini değiştirmeden önce bekleyen şifreli ayrılma isteğini sonuçlandır.",
    "Resolve the pending encrypted leave request before changing group roles.": "Grup rollerini değiştirmeden önce bekleyen şifreli ayrılma isteğini sonuçlandır.",
    "Resolve the pending encrypted leave request before removing another member.": "Başka bir üyeyi çıkarmadan önce bekleyen şifreli ayrılma isteğini sonuçlandır.",
    "The encrypted leave request is missing or has already been handled.": "Şifreli ayrılma isteği bulunamadı veya daha önce işlendi.",
    "The encrypted group must have exactly one owner to approve a leave request.": "Ayrılma isteğini onaylamak için grupta tam olarak bir sahip olmalı.",
    "Transfer group ownership before leaving the encrypted group.": "Şifreli gruptan ayrılmadan önce grup sahipliğini devret.",
    "This encrypted group can only be changed from a device that has joined its MLS group.": "Bu şifreli grup yalnızca MLS grubuna katılmış bir cihazdan değiştirilebilir.",
    "Remove this member from the encrypted group? GhostChat will send a signed MLS Commit to remaining members and they will need to verify the changed group security code.": "Bu üye şifreli gruptan çıkarılsın mı? GhostChat kalan üyelere imzalı bir MLS Commit gönderecek; üyelerin değişen grup güvenlik kodunu doğrulaması gerekecek.",
    "Finish the pending encrypted membership update before starting another one.": "Yeni bir işlem başlatmadan önce bekleyen şifreli grup güncellemesini tamamla.",
    "The saved encrypted removal is invalid.": "Kaydedilmiş şifreli üye çıkarma işlemi geçersiz.",
    "The saved encrypted membership action is not supported.": "Kaydedilmiş şifreli grup üyeliği işlemi desteklenmiyor.",
    "Manage encryption devices": "Şifreleme cihazlarını yönet",
    "Encryption device": "Şifreleme cihazı",
    "Revocation blocks this encryption-device ID from claiming unclaimed key packages on this server. It does not sign out the device's app sessions, block account access, revoke packages already claimed, or remove it from existing encrypted groups.": "İptal, bu şifreleme cihazı kimliğinin sunucudaki alınmamış anahtar paketlerini almasını engeller. Cihazın uygulama oturumlarını kapatmaz, hesap erişimini engellemez, alınmış paketleri iptal etmez veya mevcut şifreli gruplardan çıkarmaz.",
    "Loading encryption devices…": "Şifreleme cihazları yükleniyor…",
    "No encryption devices registered.": "Kayıtlı şifreleme cihazı yok.",
    "Current device": "Bu cihaz",
    "Signed-in sessions": "Açık oturumlar",
    "Sign out every other GhostChat session. This device stays signed in.": "Diğer tüm GhostChat oturumlarını kapat. Bu cihaz açık kalır.",
    "Sign out other sessions": "Diğer oturumları kapat",
    "Sign out every other GhostChat session? This device will stay signed in.": "Diğer tüm GhostChat oturumları kapatılsın mı? Bu cihaz açık kalacak.",
    "Other sessions were signed out.": "Diğer oturumların kapatıldı.",
    "Other sessions could not be signed out.": "Diğer oturumlar kapatılamadı.",
    "Active": "Etkin",
    "Revoked": "İptal edildi",
    "Registered": "Kayıt tarihi",
    "Revoke device": "Cihazı iptal et",
    "Revoke this encryption device ID? It will not be able to claim unclaimed key packages on this server. Its app sessions and account access remain active, packages already claimed stay valid, and existing encrypted-group membership is unchanged.": "Bu şifreleme cihazı kimliği iptal edilsin mi? Sunucudaki alınmamış anahtar paketlerini alamaz. Uygulama oturumları ve hesap erişimi açık kalır, alınmış paketler geçerli olur ve mevcut şifreli grup üyeliği değişmez.",
    "Encryption device revoked.": "Şifreleme cihazı iptal edildi.",
    "Encryption devices could not be loaded.": "Şifreleme cihazları yüklenemedi."
  });
  Object.assign(trText, {
    "Safety": "Güvenlik", "Safety and reporting": "Güvenlik ve bildirim", "Blocking removes the friendship and stops new direct messages and calls. Existing chat history stays available. Reports are sent to and stored by this server, and its operator can read them. Only the reason and details you enter are included; chat messages and attachments are never attached automatically. GhostChat has no moderation console or guaranteed response time.": "Engellemek arkadaşlığı kaldırır ve yeni özel mesajları ile aramaları durdurur. Eski sohbet geçmişi görünür kalır. Bildirim bu sunucuya gönderilir ve kaydedilir; sunucu yöneticisi okuyabilir. Yalnızca seçtiğin neden ve yazdığın açıklama eklenir; mesajlar ve dosyalar otomatik eklenmez. GhostChat'te moderasyon paneli veya yanıt süresi garantisi yoktur.", "Block user": "Kullanıcıyı engelle", "Unblock user": "Engeli kaldır", "Report this user": "Bu kullanıcıyı bildir", "Reason": "Neden", "Choose a reason": "Bir neden seç", "Harassment": "Taciz", "Spam or scams": "Spam veya dolandırıcılık", "Threats": "Tehdit", "Other": "Diğer", "Details (optional)": "Açıklama (isteğe bağlı)", "Send report": "Bildirimi gönder", "You blocked this user. Existing chat history is still available.": "Bu kullanıcıyı engelledin. Eski sohbet geçmişi görünür kalır.", "You unblocked this user. Send a new friend request to start a new direct chat.": "Bu kullanıcının engelini kaldırdın. Yeni özel sohbet için tekrar arkadaşlık isteği gönder.", "Report sent. No chat messages were attached.": "Bildirim gönderildi. Sohbet mesajları eklenmedi."
  });
  Object.assign(trText, { "Copy app link": "Uygulama bağlantısını kopyala", "Invite link copied.": "Davet bağlantısı kopyalandı.", "Show conversations": "Sohbetleri göster", "Hide conversations": "Sohbetleri gizle" });
  Object.assign(trText, { "Motion": "Hareketler", "Full": "Tam", "Reduced": "Azaltılmış" });
  Object.assign(trText, { "Voice processing settings applied.": "Ses işleme ayarları uygulandı." });
  Object.assign(trText, {
    "Online on this device": "Bu cihazda çevrimiçi",
    "Edit profile": "Profili düzenle",
    "Start your first conversation": "İlk sohbetini başlat",
    "Your profile is ready. Add a photo, refine your name, or find someone you trust to start a conversation.": "Profilin hazır. Bir fotoğraf ekle, adını düzenle veya sohbet başlatmak için güvendiğin birini bul."
  });
  Object.assign(trText, {
    "Account options": "Hesap seçenekleri", "Main navigation": "Ana gezinme", "Call participants": "Arama katılımcıları",
    "Your profile": "Profilin", "Log out": "Çıkış yap", "Close": "Kapat", "Send message": "Mesajı gönder",
    "Write a message": "Mesaj yaz", "Microphone input level": "Mikrofon giriş seviyesi",
    "Friends, one pending request": "Arkadaşlar, 1 bekleyen istek", "Friends, {count} pending requests": "Arkadaşlar, {count} bekleyen istek",
    "Muted for you.": "Sesi senin için kapalı.", "Right-click to mute or unmute.": "Sesini kısmak veya açmak için sağ tıkla."
  });
  Object.assign(trText, {
    "Current call": "Devam eden arama", "Return to call": "Aramaya dön", "Sharing screen": "Ekran paylaşılıyor", "Requests": "İstekler",
    "Try to connect a microphone; if none is available you can keep listening muted": "Mikrofonu bağlamayı dene; mikrofon yoksa sessiz dinlemeye devam edebilirsin",
    "Turn microphone on": "Mikrofonu aç", "Mute microphone": "Mikrofonu kapat",
    "Join the call when you are ready. If no microphone is available, you can still listen muted.": "Hazır olduğunda aramaya katıl. Mikrofonun yoksa sessiz katılıp dinleyebilirsin.",
    "No microphone found. You joined muted and can still listen.": "Mikrofon bulunamadı. Sessiz katıldın; görüşmeyi dinleyebilirsin.",
    "Push to talk is on. Hold Space to speak; your microphone stays muted otherwise.": "Bas-konuş açık. Konuşmak için Boşluk tuşunu basılı tut; diğer zamanlarda mikrofonun kapalı kalır.",
    "Your microphone is off. You can still hear the call.": "Mikrofonun kapalı. Görüşmeyi dinlemeye devam edebilirsin.",
    "Your microphone is on. Use the controls above to switch devices or mute.": "Mikrofonun açık. Yukarıdan aygıtları değiştirebilir veya mikrofonunu kapatabilirsin.",
    "Choose your microphone and speaker, or join muted and listen.": "Mikrofonunu ve hoparlörünü seç veya sessiz katılıp dinle.",
    "Checking voice relay support…": "Ses aktarma desteği kontrol ediliyor…",
    "Group voice SFU is configured. Group media is relayed and not end-to-end encrypted.": "Grup sesi yapılandırılmış. Grup görüşmeleri aktarma sunucusundan geçer ve uçtan uca şifreli değildir.",
    "TURN relay is configured. Relay-only keeps your address from the other caller; the relay still handles encrypted media traffic.": "TURN aktarma sunucusu yapılandırılmış. Yalnızca aktarma modu adresini karşı taraftan gizler; şifreli ses ve görüntü aktarma sunucusundan geçer.",
    "TURN relay is not configured. Direct calls can reveal your network address to the other caller and may fail on restrictive networks.": "TURN aktarma sunucusu yapılandırılmamış. Doğrudan aramalarda karşı taraf ağ adresini görebilir; kısıtlı ağlarda bağlantı kurulamayabilir.",
    "This server is outdated for voice and screen sharing. Restart it with the latest GhostChat server build.": "Bu sunucu ses ve ekran paylaşımı için eski. Güncel GhostChat sunucusuyla yeniden başlatılmalı.",
    "Voice relay settings are unavailable. The selected server may need the latest GhostChat server build.": "Ses aktarma ayarlarına ulaşılamıyor. Seçili sunucunun güncellenmesi gerekebilir.",
    "Updates are checked automatically.": "Güncellemeler otomatik kontrol edilir.",
    "Checking for updates…": "Güncellemeler kontrol ediliyor…",
    "GitHub Releases are not connected yet. Current version: {version}.": "GitHub Releases henüz bağlı değil. Geçerli sürüm: {version}.",
    "GhostChat {version} is ready to install.": "GhostChat {version} yüklenmeye hazır.",
    "This update includes the latest GhostChat improvements.": "Bu güncelleme GhostChat'in en yeni iyileştirmelerini içerir.",
    "You’re up to date ({version}).": "Güncelsin ({version}).",
    "GhostChat is up to date.": "GhostChat güncel.",
    "Could not check for updates. Try again when you’re online.": "Güncellemeler kontrol edilemedi. Çevrimiçiyken tekrar dene.",
    "The update could not be installed.": "Güncelleme yüklenemedi.",
    "New friend request": "Yeni arkadaşlık isteği", "{name} sent you a friend request.": "{name} sana arkadaşlık isteği gönderdi."
  });
  let updateStatusKey = "Updates are checked automatically.";
  let updateStatusValues = {};
  const tr = () => (localStorage.getItem("ghostchat.language") || "en") === "tr";
  function translate(value) { return tr() ? (trText[value] || value) : value; }
  function renderUpdateStatus() {
    const statusNode = $("#updateStatus");
    if (!statusNode) return;
    statusNode.textContent = Object.entries(updateStatusValues).reduce((text, [key, value]) => text.replaceAll(`{${key}}`, value), translate(updateStatusKey));
  }
  function setUpdateStatus(key, values = {}) { updateStatusKey = key; updateStatusValues = values; renderUpdateStatus(); }
  function applyLanguage() {
    const language = tr() ? "tr" : "en";
    document.documentElement.lang = language;
    $$("[data-en][data-tr]").forEach(node => { node.textContent = node.dataset[language]; });
    $$("[data-en-label][data-tr-label]").forEach(node => node.setAttribute("aria-label", node.getAttribute(`data-${language}-label`)));
    $$("[data-en-title][data-tr-title]").forEach(node => { node.title = node.getAttribute(`data-${language}-title`); node.setAttribute("aria-label", node.title); });
    $$("[data-en-placeholder][data-tr-placeholder]").forEach(node => { node.placeholder = node.getAttribute(`data-${language}-placeholder`); });
    const sidebarToggle = $("#sidebarToggleButton");
    if (sidebarToggle) {
      const label = translate(sidebarToggle.getAttribute("aria-expanded") === "true" ? "Hide conversations" : "Show conversations");
      sidebarToggle.title = label;
      sidebarToggle.setAttribute("aria-label", label);
    }
    for (const selector of ["#languageSelect", "#authLanguageSelect"]) { const select = $(selector); if (select) select.value = language; }
    const microphoneTestButton = $("#voiceMicTestButton");
    if (microphoneTestButton && microphoneTestStream) microphoneTestButton.textContent = translate("Stop microphone test");
    const recoveryStatus = $("#settingsRecoveryStatus");
    if (recoveryStatus && settingsRecoveryStatusKey) recoveryStatus.textContent = translate(settingsRecoveryStatusKey);
    const recoveryValue = $("#settingsRecoveryValue");
    const recoveryToggle = $("#showSettingsRecoveryKey");
    if (recoveryToggle) recoveryToggle.textContent = translate(recoveryValue?.classList.contains("hidden") ? "Show recovery key" : "Hide recovery key");
    renderUpdateStatus();
  }
  let voiceClient = null;
  let groupVoiceClient = null;
  let voiceUiState = { status: "idle" };
  let groupVoiceUiState = { status: "idle", participants: [] };
  let microphoneTestStream = null;
  let microphoneTestContext = null;
  let microphoneTestFrame = 0;
  let microphoneTestGeneration = 0;
  let voiceSpeakingContext = null;
  let voiceSpeakingAnalyzers = { local: null, remote: null };
  let voiceSpeakingSources = [];
  let voiceSpeakingFrame = 0;
  let voiceSpeakingStreams = { local: null, remote: null };

  function peerMuteKey(peerId = voiceUiState.peerId) {
    return `ghostchat.voice.peerMuted.${state.base || "local"}.${peerId || ""}`;
  }
  function peerVolumeKey(peerId = voiceUiState.peerId) {
    return `ghostchat.voice.peerVolume.${state.base || "local"}.${peerId || ""}`;
  }
  function peerVolume(peerId = voiceUiState.peerId) {
    return Math.max(0, Math.min(100, Number(localStorage.getItem(peerVolumeKey(peerId)) || "100")));
  }
  function presenceFor(userId) {
    const mode = state.presence.get(userId) || "online";
    return userId !== state.user?.id && mode === "invisible" ? "offline" : mode;
  }
  function presenceLabel(userId) {
    return translate(({ online: "Online", invisible: "Invisible", dnd: "Do not disturb", offline: "Offline" })[presenceFor(userId)] || "Offline");
  }
  function addPresenceDot(node, userId) {
    if (!node || !userId) return;
    node.querySelector?.(".presence-dot")?.remove();
    const dot = document.createElement("i"); dot.className = `presence-dot ${presenceFor(userId)}`; dot.title = presenceLabel(userId); dot.setAttribute("aria-label", dot.title); node.append(dot);
  }
  function showProfilePreview(user) {
    if (!user) return;
    $("#profilePreviewName").textContent = user.display_name || "Ghost";
    $("#profilePreviewTag").textContent = `@${user.username || "ghost"}`;
    $("#profilePreviewPresence").textContent = presenceLabel(user.id);
    $("#profilePreviewPresence").className = `presence-copy ${presenceFor(user.id)}`;
    setAvatar($("#profilePreviewAvatar"), user.display_name, user.avatar_data);
    $("#profilePreviewDialog").showModal();
  }

  function groupPeerMuteKey(conversationId, accountId) {
    return `ghostchat.voice.groupMuted.${state.base || "local"}.${conversationId || ""}.${accountId || ""}`;
  }

  function isGroupPeerMuted(conversationId, accountId) {
    return localStorage.getItem(groupPeerMuteKey(conversationId, accountId)) === "true";
  }

  function setGroupPeerMuted(conversationId, accountId, muted) {
    localStorage.setItem(groupPeerMuteKey(conversationId, accountId), String(muted));
    groupVoiceClient?.setPeerMuted(accountId, muted);
  }

  function renderGroupVoiceParticipants(participants = []) {
    const container = $("#groupVoiceParticipants");
    container.replaceChildren();
    const conversationId = groupVoiceUiState.conversationId;
    for (const participant of participants) {
      const card = create("button");
      card.type = "button";
      card.className = `voice-participant-card group-voice-participant${participant.local ? " local-participant" : ""}${participant.speaking ? " is-speaking" : ""}`;
      card.setAttribute("aria-pressed", String(!!participant.mutedForYou));
      card.setAttribute("aria-label", `${participant.name}. ${translate(participant.mutedForYou ? "Muted for you." : "Right-click to mute or unmute.")}`);
      card.title = participant.local ? participant.name : translate("Right-click to mute or unmute this caller");
      const avatar = create("span");
      avatar.className = "voice-peer-avatar voice-participant-avatar";
      setAvatar(avatar, participant.name, participant.avatar);
      const copy = create("span");
      copy.className = "voice-participant-copy";
      const name = create("strong"); name.textContent = participant.local ? `${participant.name} (${translate("You")})` : participant.name;
      const speaking = create("span"); speaking.className = "voice-participant-speaking";
      const dot = create("i"); dot.className = "voice-speaking-dot";
      const label = create("span");
      label.textContent = participant.mutedForYou ? translate("Muted for you") : participant.micMuted ? translate("Mic off") : participant.speaking ? translate("Speaking") : translate("Listening");
      speaking.append(dot, label); copy.append(name, speaking); card.append(avatar, copy);
      if (participant.mutedForYou || (participant.local && participant.micMuted)) {
        const badge = create("span"); badge.className = "voice-participant-muted";
        badge.textContent = translate(participant.local ? "Mic off" : "Muted for you"); card.append(badge);
      }
      if (!participant.local) {
        const toggle = () => setGroupPeerMuted(conversationId, participant.id, !isGroupPeerMuted(conversationId, participant.id));
        card.addEventListener("contextmenu", event => { event.preventDefault(); toggle(); });
        card.addEventListener("keydown", event => { if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) { event.preventDefault(); toggle(); } });
      }
      container.append(card);
    }
  }

  function isPeerAudioMuted(peerId = voiceUiState.peerId) {
    return !!peerId && localStorage.getItem(peerMuteKey(peerId)) === "true";
  }

  function renderPeerMuteState() {
    const muted = isPeerAudioMuted();
    const badge = $("#voicePeerMutedBadge");
    const card = $("#voicePeerCard");
    if (badge) badge.classList.toggle("hidden", !muted);
    if (card) {
      card.title = translate("Right-click to mute or unmute this caller");
      card.setAttribute("aria-label", `${$("#voicePeerName").textContent}. ${card.title}`);
      card.setAttribute("aria-expanded", String(!$("#voiceParticipantMenu").classList.contains("hidden")));
    }
    const action = $("#voiceParticipantMuteAction");
    if (action) action.textContent = translate(muted ? "Unmute their audio" : "Mute their audio");
    const volume = peerVolume();
    $("#voiceParticipantVolume").value = String(volume);
    $("#voiceParticipantVolumeValue").textContent = `${volume}%`;
  }

  function closeVoiceParticipantMenu({ restoreFocus = false } = {}) {
    $("#voiceParticipantMenu").classList.add("hidden");
    renderPeerMuteState();
    if (restoreFocus) $("#voicePeerCard").focus({ preventScroll: true });
  }

  function openVoiceParticipantMenu(event) {
    event.preventDefault();
    if (!voiceUiState.peerId || voiceUiState.status === "idle") return;
    const menu = $("#voiceParticipantMenu");
    renderPeerMuteState();
    menu.classList.remove("hidden");
    const cardRect = $("#voicePeerCard").getBoundingClientRect();
    const x = event.clientX || cardRect.left + cardRect.width / 2;
    const y = event.clientY || cardRect.bottom;
    menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - menu.offsetHeight - 8))}px`;
    renderPeerMuteState();
    $("#voiceParticipantMuteAction").focus({ preventScroll: true });
  }

  function disconnectVoiceSpeakingDetection() {
    if (voiceSpeakingFrame) cancelAnimationFrame(voiceSpeakingFrame);
    voiceSpeakingFrame = 0;
    for (const source of voiceSpeakingSources) {
      try { source.disconnect(); } catch {}
    }
    voiceSpeakingSources = [];
    if (voiceSpeakingContext && voiceSpeakingContext.state !== "closed") void voiceSpeakingContext.close().catch(() => {});
    voiceSpeakingContext = null;
    voiceSpeakingAnalyzers = { local: null, remote: null };
    voiceSpeakingStreams = { local: null, remote: null };
    $("#voiceLocalCard").classList.remove("is-speaking");
    $("#voicePeerCard").classList.remove("is-speaking");
  }

  function createSpeakingAnalyser(stream, context) {
    if (!stream?.getAudioTracks().some(track => track.readyState === "live")) return null;
    const source = context.createMediaStreamSource(stream);
    const analyser = context.createAnalyser();
    const silentOutput = context.createGain();
    analyser.fftSize = 512;
    silentOutput.gain.value = 0;
    source.connect(analyser);
    analyser.connect(silentOutput);
    silentOutput.connect(context.destination);
    voiceSpeakingSources.push(source, analyser, silentOutput);
    return { analyser, samples: new Float32Array(analyser.fftSize) };
  }

  function syncVoiceSpeakingDetection(voice) {
    if (voice.status === "idle") {
      disconnectVoiceSpeakingDetection();
      return;
    }
    const localStream = voice.localAudioStream || null;
    const remoteStream = voice.remoteAudioStream || null;
    if (voiceSpeakingContext && localStream === voiceSpeakingStreams.local && remoteStream === voiceSpeakingStreams.remote) return;
    disconnectVoiceSpeakingDetection();
    if (!localStream && !remoteStream) return;
    const AudioContextType = window.AudioContext || window.webkitAudioContext;
    if (!AudioContextType) return;
    try {
      voiceSpeakingContext = new AudioContextType();
      voiceSpeakingStreams = { local: localStream, remote: remoteStream };
      voiceSpeakingAnalyzers = {
        local: createSpeakingAnalyser(localStream, voiceSpeakingContext),
        remote: createSpeakingAnalyser(remoteStream, voiceSpeakingContext),
      };
      void voiceSpeakingContext.resume().catch(() => {});
    } catch {
      disconnectVoiceSpeakingDetection();
      return;
    }
    let lastLocalVoice = 0;
    let lastRemoteVoice = 0;
    let lastUpdate = 0;
    const readLevel = entry => {
      if (!entry) return 0;
      entry.analyser.getFloatTimeDomainData(entry.samples);
      let sum = 0;
      for (const sample of entry.samples) sum += sample * sample;
      return Math.sqrt(sum / entry.samples.length);
    };
    const tick = now => {
      if (!voiceSpeakingContext) return;
      if (now - lastUpdate >= 90) {
        lastUpdate = now;
        const localLevel = readLevel(voiceSpeakingAnalyzers.local);
        const remoteLevel = readLevel(voiceSpeakingAnalyzers.remote);
        if (localLevel > 0.012) lastLocalVoice = now;
        if (remoteLevel > 0.012) lastRemoteVoice = now;
        const localSpeaking = voiceUiState.micAvailable && !voiceUiState.micMuted && now - lastLocalVoice < 260;
        const remoteSpeaking = now - lastRemoteVoice < 260;
        $("#voiceLocalCard").classList.toggle("is-speaking", localSpeaking);
        $("#voicePeerCard").classList.toggle("is-speaking", remoteSpeaking);
        $("#voiceLocalSpeaking").textContent = translate(localSpeaking ? "Speaking" : !voiceUiState.micAvailable ? "No microphone" : voiceUiState.micMuted ? (voiceUiState.pushToTalk && !voiceUiState.micManuallyMuted ? "Hold Space" : "Mic off") : "Microphone on");
        $("#voicePeerSpeaking").textContent = translate(isPeerAudioMuted() ? "Muted for you" : remoteSpeaking ? "Speaking" : "Listening");
      }
      voiceSpeakingFrame = requestAnimationFrame(tick);
    };
    voiceSpeakingFrame = requestAnimationFrame(tick);
  }

  async function stopMicrophoneTest(statusKey = null) {
    microphoneTestGeneration += 1;
    if (microphoneTestFrame) cancelAnimationFrame(microphoneTestFrame);
    microphoneTestFrame = 0;
    microphoneTestStream?.getTracks().forEach(track => track.stop());
    microphoneTestStream = null;
    if (microphoneTestContext && microphoneTestContext.state !== "closed") await microphoneTestContext.close().catch(() => {});
    microphoneTestContext = null;
    const button = $("#voiceMicTestButton");
    if (button) {
      button.disabled = false;
      button.setAttribute("aria-pressed", "false");
      button.textContent = translate("Test microphone");
    }
    const meter = $("#voiceMicLevelMeter");
    const fill = $("#voiceMicLevelFill");
    const label = $("#voiceMicLevelLabel");
    if (meter) meter.setAttribute("aria-valuenow", "0");
    if (fill) fill.style.width = "0%";
    if (label) label.textContent = "0%";
    const status = $("#voiceMicTestStatus");
    if (status) status.textContent = translate(statusKey || "Press test and speak into your microphone.");
  }

  async function toggleMicrophoneTest() {
    if (microphoneTestStream) {
      await stopMicrophoneTest("Microphone test stopped.");
      return;
    }
    const button = $("#voiceMicTestButton");
    const generation = ++microphoneTestGeneration;
    button.disabled = true;
    try {
      if (!navigator.mediaDevices?.getUserMedia) throw new Error("Microphone access is not available in this app context.");
      const settings = voiceSettings();
      const audio = {
        echoCancellation: $("#voiceEchoCancellation").checked,
        noiseSuppression: $("#voiceNoiseSuppression").checked,
        autoGainControl: $("#voiceAutoGain").checked,
      };
      if (settings.microphoneId) audio.deviceId = { exact: settings.microphoneId };
      const stream = await navigator.mediaDevices.getUserMedia({ audio, video: false });
      if (generation !== microphoneTestGeneration || !$("#settingsDialog").open) {
        stream.getTracks().forEach(track => track.stop());
        return;
      }
      microphoneTestStream = stream;
      const AudioContextType = window.AudioContext || window.webkitAudioContext;
      if (!AudioContextType) throw new Error("Live microphone testing is not supported by this app runtime.");
      microphoneTestContext = new AudioContextType();
      await microphoneTestContext.resume();
      const analyser = microphoneTestContext.createAnalyser();
      analyser.fftSize = 512;
      const source = microphoneTestContext.createMediaStreamSource(stream);
      const silentOutput = microphoneTestContext.createGain();
      silentOutput.gain.value = 0;
      source.connect(analyser);
      analyser.connect(silentOutput);
      silentOutput.connect(microphoneTestContext.destination);
      const samples = new Float32Array(analyser.fftSize);
      let displayedLevel = 0;
      const updateMeter = () => {
        if (generation !== microphoneTestGeneration || !microphoneTestStream) return;
        analyser.getFloatTimeDomainData(samples);
        let sum = 0;
        for (const sample of samples) sum += sample * sample;
        const rms = Math.sqrt(sum / samples.length);
        const decibels = 20 * Math.log10(Math.max(rms, 0.00001));
        const target = Math.max(0, Math.min(100, Math.round((decibels + 60) * (100 / 60))));
        displayedLevel = Math.round(displayedLevel + (target - displayedLevel) * 0.35);
        $("#voiceMicLevelFill").style.width = `${displayedLevel}%`;
        $("#voiceMicLevelLabel").textContent = `${displayedLevel}%`;
        $("#voiceMicLevelMeter").setAttribute("aria-valuenow", String(displayedLevel));
        $("#voiceMicTestStatus").textContent = translate(rms > 0.004 ? "Microphone is picking up sound." : "Speak into your microphone.");
        microphoneTestFrame = requestAnimationFrame(updateMeter);
      };
      button.disabled = false;
      button.setAttribute("aria-pressed", "true");
      button.textContent = translate("Stop microphone test");
      $("#voiceMicTestStatus").textContent = translate("Speak into your microphone.");
      stream.getAudioTracks().forEach(track => track.addEventListener("ended", () => {
        if (generation === microphoneTestGeneration) void stopMicrophoneTest("Microphone test stopped.");
      }, { once: true }));
      updateMeter();
    } catch (error) {
      if (generation === microphoneTestGeneration) {
        await stopMicrophoneTest();
        toast(error.message || "Could not start microphone test.", true);
      }
    } finally {
      if (generation === microphoneTestGeneration) button.disabled = false;
    }
  }

  function voiceSettings() {
    return {
      microphoneId: localStorage.getItem("ghostchat.voice.microphone") || "",
      speakerId: localStorage.getItem("ghostchat.voice.speaker") || "",
      echoCancellation: localStorage.getItem("ghostchat.voice.echo") !== "false",
      noiseSuppression: localStorage.getItem("ghostchat.voice.noise") !== "false",
      autoGainControl: localStorage.getItem("ghostchat.voice.gain") !== "false",
      voiceFocus: localStorage.getItem("ghostchat.voice.focus") !== "false",
      voiceFocusSensitivity: Number(localStorage.getItem("ghostchat.voice.focusSensitivity") || "36"),
      pushToTalk: localStorage.getItem("ghostchat.voice.ptt") === "true",
      shareSystemAudio: localStorage.getItem("ghostchat.voice.systemAudio") === "true",
      shareQuality: localStorage.getItem("ghostchat.share.quality") || "balanced",
      relayOnly: localStorage.getItem("ghostchat.voice.relayOnly") === "true",
      volume: Number(localStorage.getItem("ghostchat.voice.volume") || "100"),
      speakerMuted: localStorage.getItem("ghostchat.voice.speakerMuted") === "true",
    };
  }

  function persistVoiceSettings() {
    localStorage.setItem("ghostchat.voice.microphone", $("#voiceMicSelect").value);
    localStorage.setItem("ghostchat.voice.speaker", $("#voiceSpeakerSelect").value);
    localStorage.setItem("ghostchat.voice.echo", String($("#voiceEchoCancellation").checked));
    localStorage.setItem("ghostchat.voice.noise", String($("#voiceNoiseSuppression").checked));
    localStorage.setItem("ghostchat.voice.gain", String($("#voiceAutoGain").checked));
    localStorage.setItem("ghostchat.voice.focus", String($("#voiceFocus").checked));
    localStorage.setItem("ghostchat.voice.focusSensitivity", $("#voiceFocusSensitivity").value);
    localStorage.setItem("ghostchat.voice.ptt", String($("#voicePushToTalk").checked));
    localStorage.setItem("ghostchat.voice.systemAudio", String($("#voiceShareAudio").checked));
    localStorage.setItem("ghostchat.share.quality", $("#voiceShareQuality").value);
    localStorage.setItem("ghostchat.voice.relayOnly", String($("#voiceRelayOnly").checked));
    localStorage.setItem("ghostchat.voice.volume", $("#voiceVolume").value);
  }

  function populateVoiceSettings() {
    const settings = voiceSettings();
    $("#voiceEchoCancellation").checked = settings.echoCancellation;
    $("#voiceNoiseSuppression").checked = settings.noiseSuppression;
    $("#voiceAutoGain").checked = settings.autoGainControl;
    $("#voiceFocus").checked = settings.voiceFocus;
    $("#voiceFocusSensitivity").value = String(Math.max(1, Math.min(100, settings.voiceFocusSensitivity || 36)));
    $("#voicePushToTalk").checked = settings.pushToTalk;
    $("#voiceShareAudio").checked = settings.shareSystemAudio;
    $("#voiceShareQuality").value = ["balanced", "detail", "motion", "saver"].includes(settings.shareQuality) ? settings.shareQuality : "balanced";
    $("#voiceRelayOnly").checked = settings.relayOnly;
    $("#voiceVolume").value = String(Math.max(0, Math.min(100, settings.volume)));
    const micSelects = [$("#voiceMicSelect"), $("#voiceCallMicSelect")];
    const speakerSelects = [$("#voiceSpeakerSelect"), $("#voiceCallSpeakerSelect")];
    const canSelectSpeaker = typeof $("#remoteAudio").setSinkId === "function";
    speakerSelects.forEach(select => { select.disabled = !canSelectSpeaker; });
    for (const select of micSelects) {
      if (settings.microphoneId && ![...select.options].some(option => option.value === settings.microphoneId)) select.add(new Option("Previously selected microphone", settings.microphoneId));
      select.value = settings.microphoneId;
    }
    for (const select of speakerSelects) {
      if (settings.speakerId && ![...select.options].some(option => option.value === settings.speakerId)) select.add(new Option("Previously selected speaker", settings.speakerId));
      select.value = settings.speakerId;
    }
    void checkVoiceRelaySupport();
  }

  async function checkVoiceRelaySupport() {
    const status = $("#voiceNetworkStatus");
    const relay = $("#voiceRelayOnly");
    status.textContent = translate("Checking voice relay support…");
    try {
      const config = await request("/v1/voice/ice");
      state.groupVoiceAvailable = config.group_voice_available === true;
      const groupStatus = $("#groupVoiceNetworkStatus");
      if (groupStatus) groupStatus.textContent = state.groupVoiceAvailable
        ? translate("Group voice SFU is configured. Group media is relayed and not end-to-end encrypted.")
        : translate("Group voice is not configured on this server. Configure LiveKit in the server environment.");
      renderVoiceControls();
      const enabled = !!config.turn_available;
      relay.disabled = !enabled;
      status.textContent = translate(enabled
        ? "TURN relay is configured. Relay-only keeps your address from the other caller; the relay still handles encrypted media traffic."
        : "TURN relay is not configured. Direct calls can reveal your network address to the other caller and may fail on restrictive networks.");
    } catch (error) {
      state.groupVoiceAvailable = false;
      const groupStatus = $("#groupVoiceNetworkStatus");
      if (groupStatus) groupStatus.textContent = translate(error.status === 404 ? "Group voice is not configured on this server. Configure LiveKit in the server environment." : "Group voice status could not be checked.");
      renderVoiceControls();
      relay.disabled = true;
      status.textContent = translate(error.status === 404
        ? "This server is outdated for voice and screen sharing. Restart it with the latest GhostChat server build."
        : "Voice relay settings are unavailable. The selected server may need the latest GhostChat server build.");
    }
  }

  async function refreshAudioDevices({ requestPermission = true, quiet = false } = {}) {
    const buttons = [$("#refreshAudioDevices"), $("#voiceRefreshDevices")].filter(Boolean);
    buttons.forEach(button => { button.disabled = true; });
    try {
      if (!navigator.mediaDevices?.enumerateDevices) throw new Error("Audio devices are not available in this app context.");
      let permissionError = "";
      if (requestPermission && navigator.mediaDevices.getUserMedia) {
        try {
          const temporaryStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
          temporaryStream.getTracks().forEach(track => track.stop());
        } catch (error) { permissionError = error.message || "Microphone permission is unavailable."; }
      }
      const devices = await navigator.mediaDevices.enumerateDevices();
      const selectedMic = voiceSettings().microphoneId; const selectedSpeaker = voiceSettings().speakerId;
      const microphones = devices.filter(device => device.kind === "audioinput");
      function fill(select, kind, selected, fallback) {
        if (!select) return;
        const options = devices.filter(device => device.kind === kind);
        select.replaceChildren(new Option(translate("System default"), ""));
        options.forEach((device, index) => select.add(new Option(device.label || `${fallback} ${index + 1}`, device.deviceId)));
        select.value = options.some(device => device.deviceId === selected) ? selected : "";
      }
      for (const id of ["#voiceMicSelect", "#voiceCallMicSelect"]) fill($(id), "audioinput", selectedMic, "Microphone");
      for (const id of ["#voiceSpeakerSelect", "#voiceCallSpeakerSelect"]) fill($(id), "audiooutput", selectedSpeaker, "Speaker");
      const notice = $("#voiceDeviceNotice");
      if (!microphones.length) notice.textContent = "No microphone detected. Calls can still be joined muted so you can listen.";
      else if (permissionError && voiceUiState.status !== "idle") notice.textContent = "Microphone access is unavailable. You can stay muted or allow access and try again.";
      if (!quiet) {
        if (!microphones.length) toast("No microphone found. You can still join calls muted.");
        else toast("Audio devices refreshed.");
      }
    } catch (error) {
      if (!quiet) toast(error.message || "Could not read audio devices.", true);
    } finally { buttons.forEach(button => { button.disabled = false; }); }
  }

  async function applyPlaybackSettings() {
    const audio = $("#remoteAudio");
    if (!audio) return;
    const settings = voiceSettings();
    audio.volume = (Math.max(0, Math.min(100, settings.volume)) / 100) * (peerVolume() / 100);
    audio.muted = settings.speakerMuted || isPeerAudioMuted();
    if (typeof audio.setSinkId === "function") {
      try { await audio.setSinkId(settings.speakerId); }
      catch {
        try { await audio.setSinkId(""); } catch {}
        localStorage.setItem("ghostchat.voice.speaker", "");
        $("#voiceSpeakerSelect").value = "";
        $("#voiceCallSpeakerSelect").value = "";
        toast("That speaker is unavailable. The system default will be used.", true);
      }
    }
    groupVoiceClient?.applyPlaybackSettings();
    void groupVoiceClient?.setSpeakerDevice(settings.speakerId).catch(error => console.warn("Could not switch group voice output", error));
  }

  function toast(message, error = false) {
    const node = $("#toast"); node.textContent = translate(message); node.classList.toggle("error", error); node.classList.add("show");
    clearTimeout(toast.timer); toast.timer = setTimeout(() => node.classList.remove("show"), 3000);
  }
  function friendRequestsStorageKey() { return `ghostchat.seenFriendRequests.${state.base || "local"}.${state.user?.id || ""}`; }
  async function sendDesktopNotification(title, body) {
    if (localStorage.getItem("ghostchat.desktopNotifications") === "false") return;
    try { await invoke("show_desktop_notification", { title, body }); }
    catch (error) { console.warn("Windows notification could not be shown", error); }
  }
  function updateFriendRequestBadge() {
    const badge = $("#friendRequestBadge");
    if (!badge) return;
    const count = state.pending?.length || 0;
    badge.classList.toggle("hidden", count === 0);
    badge.textContent = count > 9 ? "9+" : String(count);
    const requestLabel = count === 1
      ? translate("Friends, one pending request")
      : count > 1
        ? translate("Friends, {count} pending requests").replace("{count}", String(count))
        : translate("Friends");
    $("#friendsNavButton")?.setAttribute("aria-label", requestLabel);
  }
  async function checkForUpdates(quiet = false) {
    const statusNode = $("#updateStatus");
    const checkButton = $("#checkUpdates");
    setUpdateStatus("Checking for updates…");
    if (checkButton) checkButton.disabled = true;
    try {
      const result = await invoke("check_for_update");
      if (!result.configured) {
        setUpdateStatus("GitHub Releases are not connected yet. Current version: {version}.", { version: result.currentVersion });
        $("#installUpdate")?.classList.add("hidden");
        return;
      }
      if (result.available) {
        const newlyAvailable = state.updateNotifiedVersion !== result.version;
        state.updateVersion = result.version;
        state.updateNotifiedVersion = result.version;
        setUpdateStatus("GhostChat {version} is ready to install.", { version: result.version });
        const notes = $("#updateNotes");
        if (notes) {
          notes.textContent = result.notes || translate("This update includes the latest GhostChat improvements.");
          notes.classList.remove("hidden");
        }
        $("#installUpdate")?.classList.remove("hidden");
        if (newlyAvailable) toast(translate("GhostChat {version} is ready to install.").replace("{version}", result.version));
      } else {
        state.updateVersion = null;
        state.updateNotifiedVersion = null;
        setUpdateStatus("You’re up to date ({version}).", { version: result.currentVersion });
        $("#updateNotes")?.classList.add("hidden");
        $("#installUpdate")?.classList.add("hidden");
        if (!quiet) toast(translate("GhostChat is up to date."));
      }
    } catch (error) {
      setUpdateStatus("Could not check for updates. Try again when you’re online.");
      if (!quiet) toast(error.message || translate("Could not check for updates. Try again when you’re online."), true);
    } finally {
      if (checkButton) checkButton.disabled = false;
    }
  }
  async function startupUpdateCheck() {
    const gate = $("#startupUpdateGate");
    const title = $("#startupUpdateTitle");
    const status = $("#startupUpdateStatus");
    const retry = $("#retryStartupUpdate");
    gate.classList.remove("hidden"); retry.classList.add("hidden");
    title.textContent = translate("Checking for updates"); status.textContent = translate("Getting the latest version ready…");
    $("#startupUpdateProgress").style.width = "24%";
    try {
      if (!invoke) { gate.classList.add("hidden"); return true; }
      const result = await invoke("check_for_update");
      if (!result.configured || !result.available) { gate.classList.add("hidden"); return true; }
      state.updateVersion = result.version;
      title.textContent = translate("Update required"); status.textContent = `${translate("This app version is out of date.")} GhostChat ${result.version} ${translate("is downloading")}…`;
      $("#startupUpdateProgress").style.width = "42%";
      await invoke("install_update", { expectedVersion: result.version });
      return false;
    } catch (error) {
      title.textContent = translate("Update failed"); status.textContent = error.message || translate("Could not check or install the required update.");
      $("#startupUpdateProgress").style.width = "100%"; retry.classList.remove("hidden");
      return false;
    }
  }
  $("#retryStartupUpdate").onclick = () => { $("#retryStartupUpdate").disabled = true; void boot().finally(() => { $("#retryStartupUpdate").disabled = false; }); };
  async function installUpdate() {
    if (!state.updateVersion) return;
    const button = $("#installUpdate");
    button.disabled = true;
    setUpdateStatus("Preparing the update…");
    try { await invoke("install_update", { expectedVersion: state.updateVersion }); }
    catch (error) {
      setUpdateStatus(error.message || "The update could not be installed.");
      button.disabled = false;
      toast(error.message || translate("The update could not be installed."), true);
    }
  }
  function normalizedBase(value) { return value.replace(/\/+$/, ""); }
  function isLocalServer(value) {
    try {
      const { hostname } = new URL(value);
      return hostname === "localhost" || hostname === "::1" || /^127(?:\.\d{1,3}){3}$/.test(hostname);
    } catch { return false; }
  }
  function parseServerAddress(value) {
    const input = value.trim();
    if (!input) return "";
    const hasScheme = /^[a-z][a-z\d+.-]*:\/\//i.test(input);
    const isLocal = /^(localhost|127(?:\.\d{1,3}){3})(?::\d+)?(?:\/|$)/i.test(input);
    const url = new URL(hasScheme ? input : `${isLocal ? "http" : "https"}://${input}`);
    if (url.username || url.password || url.search || url.hash || (url.pathname !== "/" && url.pathname !== "")) throw new Error("Enter only the server address, without a path or extra URL details.");
    if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1"].includes(url.hostname))) throw new Error("Remote servers must use HTTPS.");
    return normalizedBase(url.origin);
  }
  function isLegacyDefaultServer(value) {
    try { return parseServerAddress(value) === LEGACY_DEFAULT_REMOTE_SERVER; }
    catch { return false; }
  }
  function activeBase() { return state.base || state.localBase; }
  async function serverKey(base) {
    const data = new TextEncoder().encode(normalizedBase(base));
    const digest = await crypto.subtle.digest("SHA-256", data);
    return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
  }
  async function migrateServerCredentials(from, to) {
    try {
      const oldKey = await serverKey(from), newKey = await serverKey(to);
      const [oldToken, newToken] = await Promise.all([
        invoke("credential_get", { serverKey: oldKey }), invoke("credential_get", { serverKey: newKey })
      ]);
      if (oldToken && !newToken) await invoke("credential_set", { serverKey: newKey, token: oldToken });
      const [oldRecovery, newRecovery] = await Promise.all([
        invoke("credential_get_pending_recovery", { serverKey: oldKey }), invoke("credential_get_pending_recovery", { serverKey: newKey })
      ]);
      if (oldRecovery && !newRecovery) await invoke("credential_set_pending_recovery", { serverKey: newKey, token: oldRecovery });
      const [oldSavedRecovery, newSavedRecovery] = await Promise.all([
        invoke("credential_get_recovery", { serverKey: oldKey }), invoke("credential_get_recovery", { serverKey: newKey })
      ]);
      if (oldSavedRecovery && !newSavedRecovery) await invoke("credential_set_recovery", { serverKey: newKey, token: oldSavedRecovery });
    } catch (error) { console.warn("Could not migrate saved credentials to the default server", error); }
  }
  async function request(path, options = {}) {
    const headers = new Headers(options.headers || {});
    headers.set("Content-Type", "application/json");
    if (state.token) headers.set("Authorization", `Bearer ${state.token}`);
    let response;
    try { response = await fetch(`${activeBase()}${path}`, { ...options, headers }); }
    catch { throw new Error("Could not reach the selected server. Check its address and connection."); }
    if (response.status === 204) return null;
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(body.error || `Request failed (${response.status}).`);
      error.status = response.status;
      throw error;
    }
    return body;
  }
  async function storeToken(token) {
    state.token = token;
    try { await invoke("credential_set", { serverKey: await serverKey(activeBase()), token }); }
    catch (error) { console.warn("Credential Manager unavailable", error); toast("Session active, but Windows could not save it. Keep your recovery key safe.", true); }
  }
  async function restoreToken() {
    try { state.token = await invoke("credential_get", { serverKey: await serverKey(activeBase()) }) || ""; }
    catch { state.token = ""; }
  }
  async function clearToken() {
    clearAttachmentUrls();
    const previous = state.token; state.token = "";
    try { if (previous) await invoke("credential_delete", { serverKey: await serverKey(activeBase()) }); } catch {}
  }
  async function savePendingRecovery(secret) {
    state.pendingRecovery = secret;
    try { await invoke("credential_set_pending_recovery", { serverKey: await serverKey(activeBase()), token: secret }); }
    catch (error) { console.warn("Could not safely retain pending recovery key", error); toast("Windows could not keep a temporary recovery backup. Save the key before closing this screen.", true); }
  }
  async function clearPendingRecovery() {
    try { await invoke("credential_delete_pending_recovery", { serverKey: await serverKey(activeBase()) }); } catch {}
    state.pendingRecovery = "";
  }
  async function saveRecoveryKey(secret) {
    state.recoveryKey = secret || "";
    if (!state.recoveryKey) return;
    await invoke("credential_set_recovery", { serverKey: await serverKey(activeBase()), token: state.recoveryKey });
  }
  async function loadRecoveryKey(inMemoryKey = state.recoveryKey) {
    state.recoveryKey = "";
    try { state.recoveryKey = await invoke("credential_get_recovery", { serverKey: await serverKey(activeBase()) }) || ""; } catch {}
    if (!state.recoveryKey) {
      try {
        state.recoveryKey = await invoke("credential_get_pending_recovery", { serverKey: await serverKey(activeBase()) }) || "";
        if (state.recoveryKey) await saveRecoveryKey(state.recoveryKey);
      } catch {}
    }
    if (!state.recoveryKey) state.recoveryKey = inMemoryKey || "";
    return state.recoveryKey;
  }
  function show(view) {
    for (const id of ["authView", "recoveryView", "appView"]) $(`#${id}`).classList.toggle("hidden", id !== view);
  }
  function updateServerLabels() {
    const remote = !!state.base;
    const label = remote ? new URL(state.base).host : "on this device";
    $("#serverModeLabel").textContent = remote ? "Selected server" : "Local server";
    $("#serverLabel").textContent = label; $("#serverAddressInput").value = state.base;
  }
  function avatarText(value = "G") { return (value.trim().replace(/^@/, "")[0] || "G").toUpperCase(); }
  function setAvatar(node, name = "G", image = null) {
    if (!node) return;
    node.replaceChildren();
    if (image) { const img = document.createElement("img"); img.src = image; img.alt = ""; img.draggable = false; node.append(img); }
    else node.textContent = avatarText(name);
  }
  function create(tag, className, text) { const node = document.createElement(tag); if (className) node.className = className; if (text !== undefined) node.textContent = translate(text); return node; }
  async function compressAvatar(file) {
    if (!file || !file.type.startsWith("image/")) throw new Error(translate("Choose an image file."));
    const bitmap = await createImageBitmap(file);
    const side = Math.min(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas"); canvas.width = 256; canvas.height = 256;
    const context = canvas.getContext("2d", { alpha: false }); context.fillStyle = "#36313f"; context.fillRect(0, 0, 256, 256);
    context.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256); bitmap.close();
    for (const quality of [0.82, 0.72, 0.62, 0.52, 0.42]) {
      const data = canvas.toDataURL("image/jpeg", quality);
      if (data.length <= 46_000) return data;
    }
    throw new Error(translate("That photo is too detailed. Choose a smaller image."));
  }
  function readImageData(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onerror = () => reject(new Error(translate("Could not read that image.")));
      reader.onload = () => resolve(String(reader.result || ""));
      reader.readAsDataURL(file);
    });
  }
  async function prepareAvatar(file) {
    const animated = file?.type === "image/gif" || file?.type === "image/webp";
    if (animated) {
      if (file.size > 80_000) throw new Error(translate("Animated profile pictures must be 80 KB or smaller."));
      return readImageData(file);
    }
    return compressAvatar(file);
  }
  async function handleAvatarFile(input, preview, save) {
    const file = input.files?.[0]; if (!file) return;
    try { const image = await prepareAvatar(file); save(image); setAvatar($(preview), state.user?.display_name || $("#createForm [name=display_name]")?.value || "G", image); }
    catch (error) { toast(error.message || translate("Could not use that photo."), true); }
    finally { input.value = ""; }
  }
  function setSignupAvatar(value) { signupAvatarData = value; }
  function applyAppearance({ persist = false } = {}) {
    const theme = $("#themeSelect").value || localStorage.getItem("ghostchat.theme") || "dark";
    const size = $("#textSize").value || localStorage.getItem("ghostchat.textSize") || "15";
    const motion = $("#motionSelect").value || localStorage.getItem("ghostchat.motion") || "full";
    document.body.dataset.theme = theme;
    document.body.classList.toggle("light", theme === "light");
    document.body.classList.toggle("reduced-motion", motion === "reduced");
    document.documentElement.style.fontSize = `${size}px`;
    const wallpaper = localStorage.getItem("ghostchat.wallpaper") || "";
    const shade = Number(localStorage.getItem("ghostchat.wallpaperShade") || "58");
    document.body.classList.toggle("has-custom-wallpaper", !!wallpaper);
    document.body.style.setProperty("--custom-wallpaper", wallpaper ? `url(\"${wallpaper}\")` : "none");
    document.body.style.setProperty("--wallpaper-shade", `${Math.min(88, Math.max(20, shade)) / 100}`);
    if (persist) {
      localStorage.setItem("ghostchat.theme", theme);
      localStorage.setItem("ghostchat.textSize", size);
      localStorage.setItem("ghostchat.motion", motion);
    }
  }
  function setTheme() {
    applyLanguage();
    $("#themeSelect").value = localStorage.getItem("ghostchat.theme") || "dark";
    $("#textSize").value = localStorage.getItem("ghostchat.textSize") || "15";
    $("#motionSelect").value = localStorage.getItem("ghostchat.motion") || "full";
    $("#themeWallpaperShade").value = localStorage.getItem("ghostchat.wallpaperShade") || "58";
    updateWallpaperStatus();
    applyAppearance();
  }
  function updateWallpaperStatus() {
    const status = $("#themeWallpaperStatus"); if (!status) return;
    const wallpaper = localStorage.getItem("ghostchat.wallpaper") || "";
    status.textContent = wallpaper ? translate("Custom wallpaper is active on this device.") : translate("No custom wallpaper selected.");
  }
  async function prepareWallpaper(file) {
    if (!file || !file.type.startsWith("image/")) throw new Error(translate("Choose an image file."));
    if ((file.type === "image/gif" || file.type === "image/webp") && file.size <= 650_000) return readImageData(file);
    const bitmap = await createImageBitmap(file); const maxWidth = 1440; const scale = Math.min(1, maxWidth / bitmap.width);
    const canvas = document.createElement("canvas"); canvas.width = Math.max(1, Math.round(bitmap.width * scale)); canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    canvas.getContext("2d", { alpha: false }).drawImage(bitmap, 0, 0, canvas.width, canvas.height); bitmap.close();
    for (const quality of [0.82, 0.72, 0.62, 0.52]) { const data = canvas.toDataURL("image/jpeg", quality); if (data.length <= 900_000) return data; }
    throw new Error(translate("That wallpaper is too detailed. Choose a smaller photo."));
  }
  function seasonalIdentity() {
    const now = new Date(); const month = now.getMonth() + 1; const day = now.getDate();
    let islamicMonth = 0; let islamicDay = 0;
    try {
      const parts = new Intl.DateTimeFormat("en-u-ca-islamic-umalqura", { month: "numeric", day: "numeric" }).formatToParts(now);
      islamicMonth = Number(parts.find(part => part.type === "month")?.value || 0);
      islamicDay = Number(parts.find(part => part.type === "day")?.value || 0);
    } catch {}
    const specialDay = month === 11 && day === 10 ? "ataturk-remembrance"
      : month === 4 && day === 23 ? "national-sovereignty"
      : month === 5 && day === 19 ? "youth-sports"
      : month === 8 && day === 30 ? "victory"
      : month === 10 && day === 29 ? "republic"
      : islamicMonth === 10 && islamicDay <= 3 ? "ramadan-feast"
      : islamicMonth === 12 && islamicDay >= 10 && islamicDay <= 13 ? "sacrifice-feast"
      : ((month === 12 && day === 31) || (month === 1 && day === 1)) ? "new-year" : "";
    document.body.dataset.specialDay = specialDay;
    const labels = {
      "ataturk-remembrance": tr() ? "10 Kasım · Atatürk’ü Anma Günü" : "10 November · Atatürk Remembrance Day",
      "national-sovereignty": tr() ? "23 Nisan" : "National Sovereignty and Children’s Day",
      "youth-sports": tr() ? "19 Mayıs" : "Commemoration of Atatürk, Youth and Sports Day",
      victory: tr() ? "30 Ağustos" : "Victory Day",
      republic: tr() ? "29 Ekim" : "Republic Day",
      "ramadan-feast": tr() ? "Ramazan Bayramı" : "Eid al-Fitr",
      "sacrifice-feast": tr() ? "Kurban Bayramı" : "Eid al-Adha",
      "new-year": tr() ? "Yeni yıl" : "New Year",
    };
    $$(".brand-mark").forEach(mark => {
      mark.title = specialDay ? `${translate("GhostChat")} · ${labels[specialDay]}` : "GhostChat";
      mark.setAttribute("aria-label", mark.title);
    });
  }
  function downloadBlob(blob, filename) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url; link.download = filename; link.style.display = "none";
    document.body.append(link); link.click(); link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1500);
  }
  function updateIdentity() {
    $("#accountName").textContent = state.user.display_name;
    $("#accountTag").textContent = `@${state.user.username}`;
    setAvatar($("#accountAvatar"), state.user.display_name, state.user.avatar_data);
    setAvatar($("#profileButton"), state.user.display_name, state.user.avatar_data);
    addPresenceDot($("#accountAvatar"), state.user.id);
    addPresenceDot($("#profileButton"), state.user.id);
    $("#profileTagLabel").textContent = `@${state.user.username}`;
  }
  async function enterApp() {
    try { state.user = await request("/v1/me"); }
    catch { await clearToken(); state.user = null; show("authView"); return; }
    resetCryptoViewState();
    show("appView"); updateIdentity(); setTheme(); seasonalIdentity(); setPage("messages");
    await refreshPresence(); await refreshBlockedUsers(); await refreshFriends(); await refreshConversations(); connectSocket(); void checkVoiceRelaySupport();
    await processPendingGroupInvites();
    renderSidebar(); showWelcome();
  }
  function renderRouteHeader() {
    if (state.page === "messages" && state.active) return;
    const routes = {
      messages: ["Your quiet corner", "Choose a friend or find someone new."],
      friends: ["Friends", "Manage requests and start a private conversation."],
      search: ["Find people", "Search for a username or user ID. Users are not listed publicly."],
      searchMessages: ["Search messages", "Search in your messages"]
    };
    const [title, subtitle] = routes[state.page] || routes.messages;
    $("#chatKindLabel").textContent = "GHOSTCHAT";
    $("#chatTitle").textContent = translate(title);
    $("#chatSubtitle").textContent = translate(subtitle);
    setAvatar($("#chatAvatar"), "G");
    $("#cryptoConversationStatus").classList.add("hidden");
    $("#enableEncryptionButton").classList.add("hidden");
    $("#typingIndicator").classList.add("hidden");
  }
  function showWelcome() {
    if (state.page !== "messages" || state.active) return;
    renderRouteHeader();
    setAvatar($("#chatAvatar"), "G"); $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden");
    $("#pageContent").innerHTML = "";
    const card = create("div", "welcome-card");
    const profile = create("button", "welcome-profile");
    profile.type = "button"; profile.title = translate("Edit profile"); profile.setAttribute("aria-label", profile.title);
    const avatar = create("span", "welcome-profile-avatar"); setAvatar(avatar, state.user?.display_name, state.user?.avatar_data);
    const profileCopy = create("span", "welcome-profile-copy");
    profileCopy.append(create("strong", "", state.user?.display_name || "Ghost"), create("span", "", `@${state.user?.username || "ghost"}`), create("small", "", "Online on this device"));
    const edit = create("span", "welcome-profile-edit", "Edit profile");
    profile.append(avatar, profileCopy, edit); profile.onclick = openSettings;
    card.append(profile, create("div", "welcome-orb", "◌"), create("h2", "", "Start your first conversation"), create("p", "", "Your profile is ready. Add a photo, refine your name, or find someone you trust to start a conversation."));
    const actions = create("div", "welcome-actions");
    const find = create("button", "button primary", "Find people"); find.onclick = () => setPage("search");
    const friends = create("button", "button secondary", "View friends"); friends.onclick = () => setPage("friends");
    actions.append(find, friends); card.append(actions); $("#pageContent").append(card); animateContent($("#pageContent")); renderVoiceControls();
  }
  function closeMobileSidebar() {
    $("#appView").classList.remove("sidebar-open");
    const button = $("#sidebarToggleButton");
    button.setAttribute("aria-expanded", "false");
    button.setAttribute("aria-label", translate("Show conversations"));
    button.title = translate("Show conversations");
  }
  function animateContent(node) {
    if (!node || document.body.classList.contains("reduced-motion")) return;
    node.classList.remove("content-enter");
    void node.offsetWidth;
    node.classList.add("content-enter");
  }
  function setPage(page) {
    sendTyping(false);
    closeMobileSidebar();
    state.page = page; state.active = page === "messages" ? state.active : null;
    if (page === "friends") state.searchTab = state.pending?.length ? "pending" : "all";
    $$(".rail-button[data-page]").forEach(button => {
      const active = button.dataset.page === page;
      button.classList.toggle("active", active);
      if (active) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    });
    const titles = { messages: "Messages", friends: "Friends", search: "Find people", searchMessages: "Search messages" };
    $("#sidebarTitle").textContent = translate(titles[page]);
    const sidebarActionLabel = translate(page === "messages" ? "Start a conversation" : "Find people");
    $("#sidebarAction").title = sidebarActionLabel;
    $("#sidebarAction").setAttribute("aria-label", sidebarActionLabel);
    if (page !== "messages") { $("#messagePanel").classList.add("hidden"); $("#pageContent").classList.remove("hidden"); renderPage(); animateContent($("#pageContent")); }
    else if (state.active) openConversation(state.active.id, false);
    else showWelcome();
    renderSidebar(); renderVoiceControls(); renderSafetyControls();
  }
  async function processPendingGroupInvites() {
    try {
      const codes = await invoke("take_pending_group_invites");
      for (const code of codes || []) {
        if (/^[A-Za-z0-9_-]{43}$/.test(code)) state.pendingGroupInviteCode = code;
      }
    } catch (error) {
      console.warn("Could not read pending group invite links", error);
      return;
    }
    if (!state.user || !state.pendingGroupInviteCode) return;
    $("#groupInviteCodeInput").value = state.pendingGroupInviteCode;
    state.pendingGroupInviteCode = null;
    if (!$("#joinGroupDialog").open) $("#joinGroupDialog").showModal();
    setTimeout(() => $("#groupInviteCodeInput").focus(), 20);
  }
  function renderSidebar() {
    const root = $("#sidebarContent"); root.replaceChildren();
    if (state.page === "messages") {
      if (!state.conversations.length) { root.append(create("div", "empty-list", "No conversations yet. Find a friend to start a chat.")); return; }
      const label = create("div", "section-label", "CONVERSATIONS"); root.append(label);
      for (const convo of state.conversations) {
        const row = create("button", `list-row${state.active?.id === convo.id ? " active" : ""}`); row.onclick = () => openConversation(convo.id);
        const avatar = create("div", "mini-avatar"); setAvatar(avatar, convo.display_name, convo.avatar_data); if (convo.kind === "direct") addPresenceDot(avatar, convo.peer_id); row.append(avatar);
        const preview = convo.last_message?.startsWith("e2e1:")
          ? translate("Encrypted conversation")
          : convo.last_message?.startsWith("file:")
          ? `📎 ${translate("File shared")}`
          : convo.last_message || (convo.kind === "group" ? `${convo.member_count} ${translate("members")}` : `@${convo.username}`);
        const copy = create("div", "list-copy"); copy.append(create("strong", "", convo.display_name), create("span", "", preview)); row.append(copy);
        if (convo.unread_count > 0) { const unread = create("span", "unread-badge", convo.unread_count > 99 ? "99+" : String(convo.unread_count)); unread.title = `${convo.unread_count} ${translate("Unread messages")}`; unread.setAttribute("aria-label", unread.title); row.append(unread); }
        root.append(row);
      }
      return;
    }
    if (state.page === "friends") {
      const requestsLabel = create("div", "section-label", "REQUESTS"); root.append(requestsLabel);
      for (const request of state.pending || []) { const row = create("button", "list-row"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, request.display_name, request.avatar_data); addPresenceDot(avatar, request.user_id); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", request.display_name), create("span", "", `@${request.username}`)); row.append(copy); root.append(row); }
      if (!(state.pending || []).length) root.append(create("div", "empty-list", "No pending requests."));
      root.append(create("div", "section-label", "YOUR FRIENDS"));
      for (const friend of state.friends) { const row = create("button", "list-row"); row.onclick = () => startConversation(friend.id); const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data); addPresenceDot(avatar, friend.id); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username} · ${presenceLabel(friend.id)}`)); row.append(copy); root.append(row); }
      if (!state.friends.length) root.append(create("div", "empty-list", "Friends you add will show up here."));
      return;
    }
    root.append(create("div", "empty-list", "Search by username or user ID. GhostChat does not publish a public directory."));
  }
  function renderPage() {
    renderRouteHeader();
    const root = $("#pageContent"); root.replaceChildren(); root.classList.remove("hidden");
    const page = create("div", "content-page");
    if (state.page === "search") {
      page.append(create("h2", "", "Find people"), create("p", "muted", "Search for a username or user ID. Users are not listed publicly."));
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = translate("Search @username or user ID"); input.setAttribute("aria-label", translate("Search people")); input.minLength = 2;
      const submit = create("button", "button primary", "Search"); submit.type = "submit"; form.append(input, submit); page.append(form);
      const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => { event.preventDefault(); if (input.value.trim().length < 2) return; results.replaceChildren(create("div", "empty-list", "Searching…")); try { const users = await request(`/v1/users/search?q=${encodeURIComponent(input.value.trim())}`); results.replaceChildren(); if (!users.length) results.append(create("div", "empty-list", "No users found. Check the spelling and try again.")); for (const user of users) { const card = create("div", "result-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, user.display_name, user.avatar_data); card.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", user.display_name), create("span", "", `@${user.username} · ${user.id.slice(0, 8)}`)); card.append(copy); const add = create("button", "button secondary", "Add friend"); add.onclick = async () => { try { await request("/v1/friends", { method: "POST", body: JSON.stringify({ user_id: user.id }) }); add.textContent = translate("Request sent"); add.disabled = true; toast(translate("Friend request sent.")); } catch (error) { toast(error.message, true); } }; card.append(add); results.append(card); } } catch (error) { results.replaceChildren(create("div", "empty-list", error.message)); } };
      setTimeout(() => input.focus(), 20);
    } else if (state.page === "friends") {
      page.append(create("h2", "", "Friends"), create("p", "muted", "Manage requests and start a private conversation."));
      const tabs = create("div", "tab-row");
      for (const [key, label] of [["all", translate("All friends")], ["pending", `${translate("Requests")}${(state.pending || []).length ? ` · ${(state.pending || []).length}` : ""}`]]) { const button = create("button", state.searchTab === key ? "active" : "", label); button.onclick = () => { state.searchTab = key; renderPage(); }; tabs.append(button); }
      page.append(tabs);
      const list = create("div", "friend-results"); page.append(list);
      if (state.searchTab === "pending") {
        if (!(state.pending || []).length) list.append(create("div", "empty-list", "No friend requests right now."));
        for (const item of state.pending || []) { const row = create("div", "request-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, item.display_name, item.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", item.display_name), create("span", "", `@${item.username}`)); row.append(copy); const actions = create("div", "request-actions"); const accept = create("button", "button primary", "Accept"); accept.onclick = () => respondFriend(item.id, "accept"); const reject = create("button", "button secondary", "Ignore"); reject.onclick = () => respondFriend(item.id, "reject"); actions.append(accept, reject); row.append(actions); list.append(row); }
      } else {
        if (!state.friends.length) list.append(create("div", "empty-list", "No friends yet. Use Find people to add someone."));
        for (const friend of state.friends) { const row = create("div", "result-card"); const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data); row.append(avatar); const copy = create("div", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`)); row.append(copy); const message = create("button", "button primary", "Message"); message.onclick = () => startConversation(friend.id); row.append(message); list.append(row); }
      }
    } else if (state.page === "searchMessages") {
      page.append(create("h2", "", "Search messages"), create("p", "muted", "Searches the 50 latest messages in each conversation on this device; your search phrase stays local."));
      const form = create("form", "search-form"); const input = create("input"); input.placeholder = translate("Search in your messages"); input.setAttribute("aria-label", translate("Search in your messages")); input.minLength = 2; const go = create("button", "button primary", "Search"); go.type = "submit"; form.append(input, go); page.append(form); const results = create("div", "search-results"); page.append(results);
      form.onsubmit = async event => {
        event.preventDefault(); const query = input.value.trim().toLocaleLowerCase(); if (query.length < 2) return;
        go.disabled = true; results.replaceChildren(create("div", "empty-list", translate("Searching locally…")));
        try {
          const matches = []; let failed = 0;
          for (let start = 0; start < state.conversations.length; start += 6) {
            const batch = state.conversations.slice(start, start + 6);
            const pages = await Promise.all(batch.map(async conversation => {
              try { return { conversation, messages: await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/messages?limit=50`) }; }
              catch { failed++; return { conversation, messages: [] }; }
            }));
            for (const { conversation, messages } of pages) for (const row of messages) {
              let searchable = row;
              if (typeof row.content === "string" && row.content.startsWith("e2e1:")) {
                try { searchable = await decodeMessageRow(row); } catch { continue; }
              }
              if (typeof searchable.content === "string" && searchable.content.toLocaleLowerCase().includes(query)) {
                matches.push({ ...searchable, conversation_name: conversation.display_name });
              }
            }
          }
          matches.sort((a, b) => new Date(b.created_at) - new Date(a.created_at)); results.replaceChildren();
          if (!matches.length) results.append(create("div", "empty-list", failed === state.conversations.length && failed > 0 ? translate("Search could not load your messages.") : translate("No matching messages.")));
          for (const row of matches) {
            const card = create("button", "result-card"); card.style.textAlign = "left"; const copy = create("div", "list-copy");
            copy.append(create("strong", "", `${row.sender_name} · ${row.conversation_name}`), create("span", "", row.content)); card.append(copy);
            card.onclick = () => { setPage("messages"); openConversation(row.conversation_id); }; results.append(card);
          }
        } catch (error) { toast(error.message, true); }
        finally { go.disabled = false; }
      };
      setTimeout(() => input.focus(), 20);
    }
    root.append(page); $("#messagePanel").classList.add("hidden");
  }
  async function refreshFriends({ notifyNew = true } = {}) {
    try {
      const [friends, pending] = await Promise.all([request("/v1/friends"), request("/v1/friends/requests")]);
      const key = friendRequestsStorageKey();
      let seen = new Set(state.seenFriendRequestIds);
      try { for (const id of JSON.parse(localStorage.getItem(key) || "[]")) seen.add(id); } catch {}
      const newRequests = pending.filter(item => !seen.has(item.id));
      state.friends = friends;
      state.pending = pending;
      state.seenFriendRequestIds = new Set(pending.map(item => item.id));
      localStorage.setItem(key, JSON.stringify([...state.seenFriendRequestIds]));
      if (notifyNew && newRequests.length) {
        if (state.page === "friends") state.searchTab = "pending";
        for (const item of newRequests) {
          const message = translate("{name} sent you a friend request.").replace("{name}", item.display_name);
          toast(message);
          void sendDesktopNotification(translate("New friend request"), message);
        }
      }
    } catch (error) { console.warn(error); }
    updateFriendRequestBadge();
    renderSidebar(); if (state.page === "friends") renderPage();
  }
  async function refreshPresence() {
    try {
      const rows = await request("/v1/me/presence");
      state.presence = new Map(rows.map(row => [row.user_id, row.mode]));
      state.presenceMode = state.presence.get(state.user?.id) || "online";
      updateIdentity();
    } catch (error) { console.warn("Could not refresh presence", error); }
  }
  async function setPresence(mode) {
    const previous = state.presenceMode;
    try {
      const result = await request("/v1/me/presence", { method: "PUT", body: JSON.stringify({ mode }) });
      state.presenceMode = result.mode; state.presence.set(result.user_id, result.mode);
      updateIdentity(); renderSidebar();
      if (state.active?.kind === "direct") await openConversation(state.active.id, false);
      toast(translate(({ online: "You are online.", invisible: "You are invisible.", dnd: "Do not disturb is on." })[result.mode]));
    } catch (error) {
      $("#presenceMode").value = previous;
      toast(error.message || "Could not update activity.", true);
    }
  }
  async function refreshConversations() {
    try { state.conversations = await request("/v1/conversations"); state.cryptoMemberIds.clear(); } catch (error) { console.warn(error); }
    renderSidebar();
  }
  async function refreshBlockedUsers() {
    try {
      const blocked = await request("/v1/me/blocks");
      state.blockedUserIds = new Set(blocked.map(user => user.id));
      renderSafetyControls(); renderVoiceControls();
    } catch (error) { console.warn(error); }
  }
  function renderSafetyControls() {
    const button = $("#userSafetyButton");
    if (!button) return;
    button.classList.toggle("hidden", !(state.page === "messages" && state.active?.kind === "direct" && state.active?.peer_id));
  }
  function openSafetyDialog() {
    const peerId = state.active?.peer_id;
    if (!peerId) return;
    const blocked = state.blockedUserIds.has(peerId);
    const blockButton = $("#safetyBlockButton");
    blockButton.textContent = translate(blocked ? "Unblock user" : "Block user");
    blockButton.classList.toggle("danger", !blocked);
    blockButton.classList.toggle("secondary", blocked);
    $("#safetyReportCategory").value = "";
    $("#safetyReportDetails").value = "";
    $("#userSafetyDialog").showModal();
  }
  async function respondFriend(id, action) {
    try { await request(`/v1/friends/requests/${encodeURIComponent(id)}/${action}`, { method: "POST", body: "{}" }); await refreshFriends(); toast(action === "accept" ? "Friend request accepted." : "Request removed."); }
    catch (error) { toast(error.message, true); }
  }
  async function startConversation(userId) {
    try { const convo = await request("/v1/conversations/direct", { method: "POST", body: JSON.stringify({ user_id: userId }) }); await refreshConversations(); setPage("messages"); await openConversation(convo.id); }
    catch (error) { toast(error.message, true); }
  }
  function openGroupDialog() {
    const list = $("#groupFriendList"); list.replaceChildren();
    for (const friend of state.friends) {
      const label = create("label", "group-friend-option");
      const checkbox = create("input"); checkbox.type = "checkbox"; checkbox.value = friend.id;
      const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data);
      const copy = create("span", "list-copy"); copy.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`));
      label.append(checkbox, avatar, copy); list.append(label);
      checkbox.addEventListener("change", updateGroupSubmitState);
    }
    if (!state.friends.length) list.append(create("div", "empty-list", translate("No friends to add yet. Add friends first.")));
    $("#groupNameInput").value = "";
    updateGroupSubmitState();
    $("#groupDialog").showModal();
  }
  function updateGroupSubmitState() {
    const count = $$("#groupFriendList input:checked").length;
    $("#submitGroup").disabled = count < 2 || count > 15 || !$("#groupNameInput").value.trim();
  }
  async function openGroupManageDialog() {
    if (!state.active || state.active.kind !== "group") return;
    $("#manageGroupName").value = state.active.display_name || "";
    $("#groupManageStatus").textContent = translate("Loading group settings…");
    $("#groupManageDialog").showModal();
    await refreshGroupManageDialog();
  }
  async function refreshGroupManageDialog() {
    const conversation = state.active;
    if (!conversation || conversation.kind !== "group") return;
    try {
      const members = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members`);
      const cryptoLock = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock`);
      const encryptedLocked = cryptoLock.locked === true;
      const me = members.find(member => member.id === state.user.id);
      if (!me) throw new Error(translate("Group settings could not be loaded."));
      const isOwner = me.role === "owner";
      const canInvite = isOwner || me.role === "admin";
      const canManageEncryptedMembers = encryptedLocked && cryptoLock.setupComplete === true && state.cryptoEnabled.has(conversation.id);
      const canAddMembers = canInvite && (!encryptedLocked || canManageEncryptedMembers);
      $("#groupNameEditor").classList.toggle("hidden", !isOwner);
      $("#groupInviteSection").classList.toggle("hidden", !canAddMembers);
      $("#encryptedGroupInviteHint").classList.toggle("hidden", !encryptedLocked || !canAddMembers);
      $("#groupInviteCodeSection").classList.toggle("hidden", encryptedLocked);
      $("#groupManageStatus").textContent = encryptedLocked
        ? isOwner
          ? translate("Transfer group ownership before leaving the encrypted group.")
          : canManageEncryptedMembers
          ? translate("Encrypted groups support verified additions, removals, and owner-approved leave requests.")
          : translate("This encrypted group can only be changed from a device that has joined its MLS group.")
        : (isOwner ? translate("Transfer group ownership before leaving the group.") : "");
      const list = $("#groupMembersList"); list.replaceChildren();
      for (const member of members) {
        const row = create("div", "group-member-row");
        const avatar = create("div", "mini-avatar"); setAvatar(avatar, member.display_name, member.avatar_data); row.append(avatar);
        const copy = create("div", "list-copy");
        copy.append(create("strong", "", member.id === state.user.id ? `${member.display_name} · ${translate("You")}` : member.display_name));
        copy.append(create("span", "", `@${member.username} · ${translate(member.role === "owner" ? "Owner" : member.role === "admin" ? "Admin" : "Member")}`));
        row.append(copy);
        const canEditRole = member.id !== state.user.id && (isOwner || (me.role === "admin" && member.role === "member"));
        if (canEditRole) {
          const role = create("select", "group-role-select");
          const options = isOwner ? ["member", "admin", "owner"] : ["member", "admin"];
          for (const optionValue of options) { const option = create("option", "", translate(optionValue === "owner" ? "Owner" : optionValue === "admin" ? "Admin" : "Member")); option.value = optionValue; role.append(option); }
          role.value = member.role;
          role.onchange = async () => {
            if (role.value === "owner" && !confirm(translate("Transfer group ownership to this person? You will become an admin."))) { role.value = member.role; return; }
            role.disabled = true;
            try {
              await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members/${encodeURIComponent(member.id)}`, { method: "PATCH", body: JSON.stringify({ role: role.value }) });
              toast(translate("Group member updated.")); await refreshConversations(); await refreshGroupManageDialog();
            } catch (error) { role.value = member.role; toast(error.message || translate("Could not update the group."), true); role.disabled = false; }
          };
          row.append(role);
        }
        const canRemove = member.id === state.user.id ? member.role !== "owner" : (isOwner && member.role !== "owner") || (me.role === "admin" && member.role === "member");
        const canRemoveEncryptedMember = encryptedLocked && canRemove && (
          (member.id === state.user.id && member.role !== "owner" && cryptoLock.setupComplete === true && state.cryptoEnabled.has(conversation.id))
          || (canManageEncryptedMembers && member.id !== state.user.id)
        );
        if ((canRemove && !encryptedLocked) || canRemoveEncryptedMember) {
          const remove = create("button", "button secondary group-remove-button", member.id === state.user.id ? translate("Leave group") : translate("Remove from group"));
          remove.onclick = async () => {
            remove.disabled = true;
            try {
              if (encryptedLocked && member.id === state.user.id) {
                if (!confirm(translate("Leave this encrypted group? The request will be sent to the owner. You will stay in the group until the owner commits the MLS update. Owners must transfer ownership before leaving."))) return;
                const pending = await invoke("crypto_get_pending_group_membership_update", { userId: state.user.id, conversationId: conversation.id });
                let leaveRequest = pending;
                if (pending && pending.action !== "leave_request") {
                  throw new Error(translate("Finish the pending encrypted membership update before starting another one."));
                }
                if (!leaveRequest) {
                  const expectedMemberIds = members.map(item => item.id).sort();
                  leaveRequest = await invoke("crypto_prepare_group_leave_proposal", {
                    userId: state.user.id, conversationId: conversation.id, expectedMemberIds,
                  });
                }
                await publishGroupMembershipUpdate(conversation, leaveRequest);
                $("#groupManageDialog").close();
                toast(translate("Leave request sent. You will remain in the group until its owner approves the encrypted update."));
              } else if (encryptedLocked) {
                if (!confirm(translate("Remove this member from the encrypted group? GhostChat will send a signed MLS Commit to remaining members and they will need to verify the changed group security code."))) return;
                const pending = await invoke("crypto_get_pending_group_membership_update", { userId: state.user.id, conversationId: conversation.id });
                if (pending) {
                  if (pending.action !== "remove" || pending.memberIds?.length !== 1 || pending.memberIds[0] !== member.id) {
                    throw new Error(translate("Finish the pending encrypted membership update before starting another one."));
                  }
                  await publishGroupMembershipUpdate(conversation, pending);
                } else {
                  const expectedMemberIds = members.map(item => item.id).sort();
                  const update = await invoke("crypto_prepare_group_removal_update", {
                    userId: state.user.id, conversationId: conversation.id,
                    expectedMemberIds, removedMemberIds: [member.id],
                  });
                  await publishGroupMembershipUpdate(conversation, update);
                }
              } else {
                await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members/${encodeURIComponent(member.id)}`, { method: "DELETE" });
              }
              if (encryptedLocked && member.id === state.user.id) {
                // The request is committed later by the group owner.
              } else if (member.id === state.user.id) {
                $("#groupManageDialog").close(); state.active = null; await refreshConversations(); showWelcome(); toast(translate("You left the group."));
              } else { toast(translate("Member removed from the group.")); await refreshConversations(); await refreshGroupManageDialog(); }
            } catch (error) { toast(translate(error.message) || translate("Could not update the group."), true); remove.disabled = false; }
          };
          row.append(remove);
        }
        list.append(row);
      }
      $("#manageGroupName").value = conversation.display_name || "";
      const currentIds = new Set(members.map(member => member.id));
      const inviteList = $("#groupInviteList"); inviteList.replaceChildren();
      const inviteable = state.friends.filter(friend => !currentIds.has(friend.id));
      const freeSlots = Math.max(0, 16 - members.length);
      for (const friend of inviteable) {
        const label = create("label", "group-friend-option"); const checkbox = create("input"); checkbox.type = "checkbox"; checkbox.value = friend.id;
        checkbox.disabled = freeSlots === 0;
        checkbox.addEventListener("change", () => {
          if ($$("#groupInviteList input:checked").length > freeSlots) {
            checkbox.checked = false; $("#groupManageStatus").textContent = translate("Groups can have up to 16 members.");
          }
        });
        const avatar = create("div", "mini-avatar"); setAvatar(avatar, friend.display_name, friend.avatar_data);
        const name = create("span", "list-copy"); name.append(create("strong", "", friend.display_name), create("span", "", `@${friend.username}`));
        label.append(checkbox, avatar, name); inviteList.append(label);
      }
      if (!inviteable.length) inviteList.append(create("div", "empty-list", translate("No friends available to invite.")));
      $("#inviteGroupFriends").disabled = !canAddMembers || !inviteable.length || freeSlots === 0 || state.cryptoMembershipPending.has(conversation.id);
      const inviteCodeList = $("#groupInvitesList"); inviteCodeList.replaceChildren();
      if (canInvite) {
        const invites = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/invites`);
        if (!invites.length) inviteCodeList.append(create("div", "empty-list", translate("No active invite codes.")));
        for (const invite of invites) {
          const row = create("div", "group-invite-row");
          const info = create("div", "list-copy");
          info.append(create("strong", "", `${translate("Uses")}: ${invite.use_count}/${invite.max_uses}`));
          const expires = new Date(invite.expires_at);
          info.append(create("span", "", `${translate("Expires")}: ${Number.isNaN(expires.getTime()) ? invite.expires_at : expires.toLocaleString()}`));
          row.append(info);
          const code = state.groupInviteCodes.get(invite.id);
          if (code) {
            const codeRow = create("div", "group-invite-code");
            const value = create("input"); value.readOnly = true; value.value = code; value.setAttribute("aria-label", translate("Invite code"));
            const copy = create("button", "button secondary", translate("Copy")); copy.type = "button"; copy.onclick = async () => { try { await navigator.clipboard.writeText(code); toast(translate("Invite code copied.")); } catch { value.select(); toast(translate("Could not copy invite code."), true); } };
            const copyLink = create("button", "button secondary", translate("Copy app link")); copyLink.type = "button"; copyLink.onclick = async () => { try { await navigator.clipboard.writeText(`ghostchat://invite/${code}`); toast(translate("Invite link copied.")); } catch { toast(translate("Could not copy invite code."), true); } };
            codeRow.append(value, copy, copyLink); row.append(codeRow);
          }
          const revoke = create("button", "button secondary group-revoke-invite", translate("Revoke")); revoke.type = "button"; revoke.onclick = async () => {
            revoke.disabled = true;
            try { await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/invites/${encodeURIComponent(invite.id)}`, { method: "DELETE" }); state.groupInviteCodes.delete(invite.id); toast(translate("Group invite revoked.")); await refreshGroupManageDialog(); }
            catch (error) { toast(error.message, true); revoke.disabled = false; }
          };
          row.append(revoke); inviteCodeList.append(row);
        }
      }
    } catch (error) {
      $("#groupManageStatus").textContent = error.message || translate("Group settings could not be loaded.");
    }
  }
  function cryptoObservedFingerprintKey(conversationId) { return `ghostchat.cryptoPeerObserved.${state.base}.${state.user?.id || ""}.${conversationId}`; }
  function cryptoTrustedFingerprintKey(conversationId) { return `ghostchat.cryptoPeerTrusted.${state.base}.${state.user?.id || ""}.${conversationId}`; }
  function cryptoEventCursorKey(conversationId) { return `ghostchat.cryptoCursor.${state.base}.${state.user?.id || ""}.${conversationId}`; }
  function cryptoBootstrapKey(conversationId) { return `ghostchat.cryptoBootstrap.${state.base}.${state.user?.id || ""}.${conversationId}`; }
  async function expectedCryptoMemberIds(conversationId) {
    const cached = state.cryptoMemberIds.get(conversationId);
    if (cached) return cached;
    const conversation = state.active?.id === conversationId ? state.active : state.conversations.find(item => item.id === conversationId);
    let ids;
    if (conversation?.kind === "direct") ids = [state.user?.id, conversation.peer_id].filter(Boolean);
    else {
      const members = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`);
      ids = members.map(member => member.id);
    }
    ids = [...new Set(ids)].sort();
    state.cryptoMemberIds.set(conversationId, ids);
    return ids;
  }
  function currentCryptoFingerprint(conversationId) {
    return state.cryptoPeerFingerprints.get(conversationId) || localStorage.getItem(cryptoObservedFingerprintKey(conversationId)) || "";
  }
  function cryptoIdentityTrusted(conversationId) {
    const fingerprint = currentCryptoFingerprint(conversationId);
    return !!fingerprint && localStorage.getItem(cryptoTrustedFingerprintKey(conversationId)) === fingerprint;
  }
  function updateCryptoStatus(conversation = state.active) {
    const button = $("#enableEncryptionButton"); const status = $("#cryptoConversationStatus");
    if (!button || !status) return;
    const encryptable = conversation?.kind === "direct" || conversation?.kind === "group";
    const localGroup = !!conversation && state.cryptoEnabled.has(conversation.id);
    const serverLocked = state.cryptoLocked.has(conversation.id);
    const setupComplete = state.cryptoSetupComplete.has(conversation.id);
    const canManageCrypto = state.cryptoCanManage.has(conversation.id);
    button.classList.toggle("hidden", !encryptable);
    status.classList.toggle("hidden", !encryptable || (!localGroup && !serverLocked));
    $("#attachFileButton")?.classList.toggle("hidden", !!encryptable && serverLocked && (!localGroup || !cryptoIdentityTrusted(conversation.id)));
    if (!encryptable) return;
    if (!localGroup) {
      button.textContent = translate(serverLocked ? (canManageCrypto && !setupComplete ? "Continue encryption setup" : "Encrypted") : "Enable encryption"); button.disabled = serverLocked && (!canManageCrypto || setupComplete);
      button.title = button.textContent; button.setAttribute("aria-label", button.textContent);
      status.textContent = serverLocked ? translate("This conversation is locked for MLS encryption. Prepare this device in Settings to read new messages; ask the person who started setup for help if it still fails.") : "";
      status.classList.toggle("warning", serverLocked); return;
    }
    const trusted = cryptoIdentityTrusted(conversation.id);
    button.textContent = translate(trusted ? "Encrypted" : "Verify identity"); button.disabled = trusted;
    button.title = button.textContent; button.setAttribute("aria-label", button.textContent);
    status.textContent = trusted
      ? translate(conversation.kind === "group"
        ? "This encrypted group uses MLS for text messages. Existing messages and previous files are not encrypted; new attachments use MLS-wrapped file keys. Membership changes require verified MLS updates."
        : "This direct message now uses MLS encryption. Messages and files sent before setup remain stored as plain text on the server; new attachments are encrypted.")
      : translate(conversation.kind === "group"
        ? "The encrypted group identity is not verified. Compare the full group code with its members before sending private messages."
        : "The encrypted identity is not verified. Compare this code with your friend before sending private messages.");
    status.classList.toggle("warning", !trusted);
  }
  function verifyPeerFingerprint(conversationId, fingerprint) {
    if (!fingerprint) return false;
    const trusted = localStorage.getItem(cryptoTrustedFingerprintKey(conversationId));
    state.cryptoPeerFingerprints.set(conversationId, fingerprint);
    localStorage.setItem(cryptoObservedFingerprintKey(conversationId), fingerprint);
    if (trusted === fingerprint) return true;
    const conversation = state.active?.id === conversationId ? state.active : state.conversations.find(item => item.id === conversationId);
    const promptText = trusted
      ? translate("The verified security code changed. Compare the old and current codes with everyone in this conversation through another trusted channel. Continue only if you expected this identity or membership change; canceling keeps new messages blocked.")
      : conversation?.kind === "group"
        ? translate("Verify the full group security code with every member through another trusted channel before continuing. The list below shows each member's device code.")
        : translate("Compare this security code with your friend through another trusted channel before continuing. Continue only if the codes match.");
    const prompt = trusted
      ? `${promptText}\n\n${translate("Previous verified code")}\n${trusted}\n\n${translate("Current security code")}\n${fingerprint}`
      : `${promptText}\n\n${fingerprint}`;
    if (!confirm(prompt)) return false;
    localStorage.setItem(cryptoTrustedFingerprintKey(conversationId), fingerprint);
    return true;
  }
  async function syncCryptoEvents(conversationId) {
    if (!state.user || !invoke) return;
    if (!state.cryptoEnabled.has(conversationId) && !state.cryptoLocked.has(conversationId)) return;
    if (state.cryptoMembershipPending.has(conversationId)) return;
    if (state.cryptoSyncing.has(conversationId)) return state.cryptoSyncing.get(conversationId);
    const task = (async () => {
      const conversation = state.active?.id === conversationId ? state.active : state.conversations.find(item => item.id === conversationId);
      const conversationIsGroup = conversation?.kind === "group";
      const expectedMemberIds = await expectedCryptoMemberIds(conversationId);
      const savedCursor = await invoke("crypto_get_event_cursor", { userId: state.user.id, conversationId });
      const localCursor = Number(localStorage.getItem(cryptoEventCursorKey(conversationId)) || 0);
      let cursor;
      if (savedCursor == null) {
        cursor = Number.isSafeInteger(localCursor) && localCursor >= 0 ? localCursor : 0;
        await invoke("crypto_advance_event_cursor", { userId: state.user.id, conversationId, sequence: cursor });
      } else {
        cursor = Number(savedCursor);
        if (!Number.isSafeInteger(cursor) || cursor < 0) throw new Error(translate("The local encrypted event cursor is invalid."));
      }
      localStorage.setItem(cryptoEventCursorKey(conversationId), String(cursor));
      let holdForVerification = false;
      for (let page = 0; page < 20; page++) {
        const events = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/crypto-events?after=${cursor}&limit=100`);
        if (!Array.isArray(events) || !events.length) break;
        for (const event of events) {
          const sequence = Number(event.sequence);
          if (!Number.isSafeInteger(sequence) || sequence <= cursor) continue;
          if (state.cryptoEnabled.has(conversationId) && !cryptoIdentityTrusted(conversationId)) {
            holdForVerification = true;
            break;
          }
          if (event.sender_id !== state.user.id) {
            let result;
            try {
              result = await invoke("crypto_process_event", {
                userId: state.user.id, conversationId, senderId: event.sender_id,
                clientMessageId: event.id, eventSequence: sequence, conversationIsGroup, expectedMemberIds, payload: event.payload,
              });
            } catch (error) {
              if (conversationIsGroup && event.recipient_id == null) {
                console.warn("Ignored an obsolete group broadcast that was not addressed to this device.", error);
                cursor = await invoke("crypto_advance_event_cursor", { userId: state.user.id, conversationId, sequence });
                localStorage.setItem(cryptoEventCursorKey(conversationId), String(cursor));
                continue;
              }
              throw error;
            }
            if (result.kind === "welcome" && result.peerIdentityFingerprint) {
              state.cryptoEnabled.add(conversationId);
              verifyPeerFingerprint(conversationId, result.peerIdentityFingerprint);
              updateCryptoStatus(state.active?.id === conversationId ? state.active : { id: conversationId, kind: conversationIsGroup ? "group" : "direct" });
              if (!cryptoIdentityTrusted(conversationId)) holdForVerification = true;
            } else if (result.kind === "commit") {
              const latestFingerprint = conversationIsGroup
                ? await invoke("crypto_group_fingerprint", { userId: state.user.id, conversationId })
                : conversation?.peer_id
                  ? await invoke("crypto_direct_peer_fingerprint", { userId: state.user.id, peerUserId: conversation.peer_id, conversationId })
                  : null;
              if (latestFingerprint) {
                state.cryptoPeerFingerprints.set(conversationId, latestFingerprint);
                localStorage.setItem(cryptoObservedFingerprintKey(conversationId), latestFingerprint);
                if (!cryptoIdentityTrusted(conversationId)) holdForVerification = true;
                updateCryptoStatus(state.active?.id === conversationId ? state.active : { id: conversationId, kind: conversationIsGroup ? "group" : "direct" });
              }
            } else if (result.kind === "leave_request" && conversationIsGroup && result.content) {
              const members = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`);
              const currentMemberIds = members.map(member => member.id).sort();
              const me = members.find(member => member.id === state.user?.id);
              if (me?.role === "owner") {
                const update = await invoke("crypto_prepare_group_leave_commit", {
                  userId: state.user.id, conversationId, operationId: result.content,
                  leavingUserId: event.sender_id, expectedMemberIds: currentMemberIds,
                });
                await publishGroupMembershipUpdate(conversation || { id: conversationId, kind: "group" }, update);
              }
            } else if (result.kind === "application" && result.content && state.active?.id === conversationId) {
              const conversation = state.active;
              appendMessage({ id: event.id, sequence, conversation_id: conversationId, sender_id: event.sender_id,
                sender_name: conversation.display_name, sender_avatar: conversation.avatar_data, content: result.content,
                created_at: event.created_at, encrypted: true });
            }
          } else {
            cursor = await invoke("crypto_advance_event_cursor", { userId: state.user.id, conversationId, sequence });
          }
          cursor = Math.max(cursor, sequence);
          localStorage.setItem(cryptoEventCursorKey(conversationId), String(cursor));
          if (holdForVerification) break;
        }
        if (holdForVerification) break;
        if (events.length < 100) break;
      }
      if (conversationIsGroup && await invoke("crypto_has_direct_group", { userId: state.user.id, conversationId })) {
        const fingerprint = await invoke("crypto_group_fingerprint", { userId: state.user.id, conversationId });
        if (fingerprint) {
          state.cryptoPeerFingerprints.set(conversationId, fingerprint);
          localStorage.setItem(cryptoObservedFingerprintKey(conversationId), fingerprint);
          if (state.active?.id === conversationId) updateCryptoStatus(state.active);
        }
      }
    })().catch(error => {
      if (state.active?.id === conversationId) toast(error.message || translate("Encryption could not be enabled for this conversation."), true);
    }).finally(() => state.cryptoSyncing.delete(conversationId));
    state.cryptoSyncing.set(conversationId, task);
    return task;
  }
  async function refreshCryptoConversation(conversation) {
    const button = $("#enableEncryptionButton"); const status = $("#cryptoConversationStatus");
    if (!["direct", "group"].includes(conversation?.kind) || !state.user || !invoke) {
      state.cryptoEnabled.delete(conversation?.id); updateCryptoStatus(conversation); return;
    }
    button.classList.remove("hidden"); button.disabled = true; button.textContent = translate("Checking…");
    try {
      if (conversation.kind === "group") {
        const cryptoLock = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock`);
        if (cryptoLock.locked) state.cryptoLocked.add(conversation.id); else state.cryptoLocked.delete(conversation.id);
        if (cryptoLock.setupComplete) state.cryptoSetupComplete.add(conversation.id); else state.cryptoSetupComplete.delete(conversation.id);
        const members = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members`);
        const memberIds = members.map(member => member.id).sort();
        state.cryptoMemberIds.set(conversation.id, memberIds);
        const canStart = !cryptoLock.locked && members.some(member => member.id === state.user.id && ["owner", "admin"].includes(member.role));
        const canResume = cryptoLock.locked && !cryptoLock.setupComplete && cryptoLock.lockedBy === state.user.id;
        if (canStart || canResume) state.cryptoCanManage.add(conversation.id); else state.cryptoCanManage.delete(conversation.id);
      } else {
        const cryptoLock = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock`);
        if (cryptoLock.locked) state.cryptoLocked.add(conversation.id); else state.cryptoLocked.delete(conversation.id);
        if (cryptoLock.setupComplete) state.cryptoSetupComplete.add(conversation.id); else state.cryptoSetupComplete.delete(conversation.id);
        if (cryptoLock.locked && !cryptoLock.setupComplete && cryptoLock.lockedBy === state.user.id) state.cryptoCanManage.add(conversation.id);
        else state.cryptoCanManage.delete(conversation.id);
      }
      let enabled = await invoke("crypto_has_direct_group", { userId: state.user.id, conversationId: conversation.id });
      if (enabled && conversation.kind === "group") {
        const membershipRecovered = await retryPendingGroupMembershipUpdate(conversation);
        if (!membershipRecovered) throw new Error(translate("Pending encrypted membership update is being retried…"));
      }
      await syncCryptoEvents(conversation.id);
      if (state.active?.id !== conversation.id) return;
      enabled = await invoke("crypto_has_direct_group", { userId: state.user.id, conversationId: conversation.id });
      if (enabled) {
        state.cryptoEnabled.add(conversation.id);
        let storedFingerprint = localStorage.getItem(cryptoObservedFingerprintKey(conversation.id));
        if (conversation.kind === "direct" && conversation.peer_id) {
          storedFingerprint = await invoke("crypto_direct_peer_fingerprint", { userId: state.user.id, peerUserId: conversation.peer_id, conversationId: conversation.id });
          if (storedFingerprint) localStorage.setItem(cryptoObservedFingerprintKey(conversation.id), storedFingerprint);
        } else if (conversation.kind === "group") {
          const currentFingerprint = await invoke("crypto_group_fingerprint", { userId: state.user.id, conversationId: conversation.id });
          if (currentFingerprint && currentFingerprint !== storedFingerprint) {
            storedFingerprint = currentFingerprint;
            localStorage.setItem(cryptoObservedFingerprintKey(conversation.id), storedFingerprint);
          }
        }
        if (storedFingerprint) state.cryptoPeerFingerprints.set(conversation.id, storedFingerprint);
        updateCryptoStatus(conversation);
        let pending = localStorage.getItem(cryptoBootstrapKey(conversation.id));
        if (!pending) {
          const savedBootstrap = await invoke("crypto_get_pending_bootstrap", { userId: state.user.id, conversationId: conversation.id });
          if (savedBootstrap) {
            pending = JSON.stringify(savedBootstrap);
            localStorage.setItem(cryptoBootstrapKey(conversation.id), pending);
          }
        }
        if (pending) void retryCryptoBootstrap(conversation);
        else void retryPendingEncryptedMessage(conversation).catch(error => toast(error.message || translate("The encrypted message is waiting to be sent. Try again when connected."), true));
      } else {
        state.cryptoEnabled.delete(conversation.id); updateCryptoStatus(conversation);
      }
    } catch (error) {
      const locked = state.cryptoLocked.has(conversation.id);
      const canManage = state.cryptoCanManage.has(conversation.id);
      const complete = state.cryptoSetupComplete.has(conversation.id);
      button.classList.remove("hidden"); button.textContent = translate(locked ? (canManage && !complete ? "Continue encryption setup" : "Encrypted") : "Enable encryption"); button.disabled = locked && (!canManage || complete);
      status.textContent = state.cryptoLocked.has(conversation.id)
        ? translate("This conversation is locked for MLS encryption. Prepare this device in Settings to read new messages; ask the person who started setup for help if it still fails.")
        : (error.message || translate("Prepare this device for encryption in Settings first."));
      status.classList.remove("hidden"); status.classList.add("warning");
    }
  }
  async function publishCryptoBootstrap(conversation, pending) {
    const endpoint = `/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-events`;
    const peerFingerprint = pending.peerFingerprint || pending.peerIdentityFingerprint;
    let welcomeRecipients = Array.isArray(pending.welcomeRecipients) ? pending.welcomeRecipients : [];
    if (!welcomeRecipients.length) {
      welcomeRecipients = conversation.kind === "group"
        ? (await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members`)).map(member => member.id).filter(id => id !== state.user.id)
        : [conversation.peer_id].filter(Boolean);
      welcomeRecipients.sort();
    }
    for (let index = 0; index < welcomeRecipients.length; index++) {
      const recipientUserId = welcomeRecipients[index];
      await request(endpoint, {
        method: "POST",
        body: JSON.stringify({
          client_event_id: `${pending.welcomeId}-${index}`,
          recipient_user_id: recipientUserId,
          payload: pending.welcomePayload,
        }),
      });
    }
    await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock/complete`, { method: "POST" });
    state.cryptoSetupComplete.add(conversation.id);
    state.cryptoCanManage.delete(conversation.id);
    await invoke("crypto_clear_pending_bootstrap", { userId: state.user.id, conversationId: conversation.id });
    localStorage.removeItem(cryptoBootstrapKey(conversation.id));
    state.cryptoEnabled.add(conversation.id);
    state.cryptoPeerFingerprints.set(conversation.id, peerFingerprint);
    localStorage.setItem(cryptoObservedFingerprintKey(conversation.id), peerFingerprint);
    localStorage.setItem(cryptoTrustedFingerprintKey(conversation.id), peerFingerprint);
    updateCryptoStatus(conversation);
    await syncCryptoEvents(conversation.id);
    await loadMessages(false);
    toast(translate("Encrypted messages are ready."));
  }
  async function publishGroupMembershipUpdate(conversation, pending) {
    const conversationId = conversation.id;
    const inFlight = state.cryptoMembershipOperations.get(conversationId);
    if (inFlight) return inFlight;
    state.cryptoMembershipPending.add(conversationId);
    const task = (async () => {
      if (pending.action === "remove") {
        if (!Array.isArray(pending.memberIds) || pending.memberIds.length !== 1) throw new Error(translate("The saved encrypted removal is invalid."));
        await request(`/v1/conversations/${encodeURIComponent(conversationId)}/crypto-membership/remove/${encodeURIComponent(pending.memberIds[0])}`, {
          method: "POST",
          body: JSON.stringify({
            operation_id: pending.operationId,
            expected_member_ids: pending.expectedMemberIds,
            commit_payload: pending.commitPayload,
            welcome_payload: "",
          }),
        });
      } else if (pending.action === "leave_request") {
        if (!Array.isArray(pending.memberIds) || pending.memberIds.length !== 1 || pending.memberIds[0] !== state.user.id) {
          throw new Error(translate("The saved encrypted leave request is invalid."));
        }
        await request(`/v1/conversations/${encodeURIComponent(conversationId)}/crypto-membership/leave`, {
          method: "POST",
          body: JSON.stringify({
            operation_id: pending.operationId,
            expected_member_ids: pending.expectedMemberIds,
            proposal_payload: pending.commitPayload,
          }),
        });
        state.cryptoMembershipPending.delete(conversationId);
        return "leave_request_pending";
      } else if (pending.action === "leave") {
        if (!Array.isArray(pending.memberIds) || pending.memberIds.length !== 1) throw new Error(translate("The saved encrypted leave commit is invalid."));
        await request(`/v1/conversations/${encodeURIComponent(conversationId)}/crypto-membership/leave/commit/${encodeURIComponent(pending.memberIds[0])}`, {
          method: "POST",
          body: JSON.stringify({
            operation_id: pending.operationId,
            expected_member_ids: pending.expectedMemberIds,
            commit_payload: pending.commitPayload,
            welcome_payload: "",
          }),
        });
      } else if (pending.action === "add") {
        await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`, {
          method: "POST",
          body: JSON.stringify({
            member_ids: pending.memberIds,
            crypto_update: {
              operation_id: pending.operationId,
              expected_member_ids: pending.expectedMemberIds,
              commit_payload: pending.commitPayload,
              welcome_payload: pending.welcomePayload,
            },
          }),
        });
      } else {
        throw new Error(translate("The saved encrypted membership action is not supported."));
      }
      const fingerprint = await invoke("crypto_finalize_group_membership_update", {
        userId: state.user.id, conversationId, operationId: pending.operationId,
      });
      state.cryptoMembershipPending.delete(conversationId);
      const members = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`);
      const memberIds = members.map(member => member.id).sort();
      state.cryptoMemberIds.set(conversationId, memberIds);
      state.cryptoEnabled.add(conversationId);
      state.cryptoPeerFingerprints.set(conversationId, fingerprint);
      localStorage.setItem(cryptoObservedFingerprintKey(conversationId), fingerprint);
      updateCryptoStatus(conversation);
      return fingerprint;
    })();
    state.cryptoMembershipOperations.set(conversationId, task);
    try {
      return await task;
    } finally {
      if (state.cryptoMembershipOperations.get(conversationId) === task) {
        state.cryptoMembershipOperations.delete(conversationId);
      }
    }
  }
  async function retryPendingGroupMembershipUpdate(conversation) {
    if (!invoke || conversation?.kind !== "group") return true;
    const pending = await invoke("crypto_get_pending_group_membership_update", {
      userId: state.user.id, conversationId: conversation.id,
    });
    if (!pending) return true;
    state.cryptoMembershipPending.add(conversation.id);
    if (state.active?.id === conversation.id) {
      $("#groupManageStatus").textContent = translate("Pending encrypted membership update is being retried…");
    }
    try {
      const result = await publishGroupMembershipUpdate(conversation, pending);
      if (result === "leave_request_pending") return true;
      return true;
    } catch (error) {
      if (state.active?.id === conversation.id) {
        $("#groupManageStatus").textContent = translate(error.message) || translate("Could not update the group.");
      }
      if (pending.action === "leave_request") {
        state.cryptoMembershipPending.delete(conversation.id);
        return true;
      }
      return false;
    }
  }
  async function retryCryptoBootstrap(conversation) {
    let raw = localStorage.getItem(cryptoBootstrapKey(conversation.id));
    if (!raw) {
      try {
        const savedBootstrap = await invoke("crypto_get_pending_bootstrap", { userId: state.user.id, conversationId: conversation.id });
        if (savedBootstrap) { raw = JSON.stringify(savedBootstrap); localStorage.setItem(cryptoBootstrapKey(conversation.id), raw); }
      } catch (error) { toast(error.message || translate("The encrypted message is waiting to be sent. Try again when connected."), true); return; }
    }
    if (!raw) return;
    try { await publishCryptoBootstrap(conversation, JSON.parse(raw)); }
    catch (error) { toast(error.message || translate("The encrypted message is waiting to be sent. Try again when connected."), true); }
  }
  async function enableEncryption() {
    const conversation = state.active; const button = $("#enableEncryptionButton");
    if (!conversation || !["direct", "group"].includes(conversation.kind) || !state.user) return;
    if (state.cryptoEnabled.has(conversation.id)) {
      const fingerprint = currentCryptoFingerprint(conversation.id);
      const verified = fingerprint ? verifyPeerFingerprint(conversation.id, fingerprint) : false;
      updateCryptoStatus(conversation);
      if (verified) await syncCryptoEvents(conversation.id);
      return;
    }
    button.disabled = true; button.textContent = translate("Setting up encryption…");
    try {
      let pending = localStorage.getItem(cryptoBootstrapKey(conversation.id));
      if (!pending) {
        const savedBootstrap = await invoke("crypto_get_pending_bootstrap", { userId: state.user.id, conversationId: conversation.id });
        if (savedBootstrap) { pending = JSON.stringify(savedBootstrap); localStorage.setItem(cryptoBootstrapKey(conversation.id), pending); }
      }
      if (pending) { await retryCryptoBootstrap(conversation); return; }
      let group;
      if (conversation.kind === "group") {
        const members = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/members`);
        const me = members.find(member => member.id === state.user.id);
        if (!me || !["owner", "admin"].includes(me.role)) throw new Error(translate("Group encryption can only be started by a group owner or admin."));
        const peers = members.filter(member => member.id !== state.user.id).sort((a, b) => a.id.localeCompare(b.id));
        if (!peers.length || peers.length > 15) throw new Error(translate("Encryption could not be enabled for this conversation."));
        const keyPackages = [];
        const deviceCodes = [];
        for (const peer of peers) {
          const claimed = await request(`/v1/users/${encodeURIComponent(peer.id)}/crypto/key-package`, {
            method: "POST", body: JSON.stringify({ client_claim_id: crypto.randomUUID() }),
          });
          const inspected = await invoke("crypto_inspect_key_package", {
            peerUserId: peer.id, peerDeviceId: claimed.device_id, payload: claimed.payload,
          });
          keyPackages.push({ userId: peer.id, deviceId: claimed.device_id, payload: claimed.payload });
          deviceCodes.push(`${peer.display_name} (@${peer.username})\n${inspected.identityFingerprint}`);
        }
        const approved = confirm(`${translate("Verify the full group security code with every member through another trusted channel before continuing. The list below shows each member's device code.")}\n\n${deviceCodes.join("\n\n")}`);
        if (!approved) { toast(translate("Encryption setup was cancelled. The one-time key packages were already claimed; members may need to publish fresh packages.")); return; }
        const roster = members.map(member => member.id).sort();
        await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock`, {
          method: "POST", body: JSON.stringify({ member_ids: roster }),
        });
        state.cryptoMemberIds.set(conversation.id, roster);
        state.cryptoLocked.add(conversation.id);
        state.cryptoSetupComplete.delete(conversation.id);
        state.cryptoCanManage.add(conversation.id);
        group = await invoke("crypto_create_group", {
          userId: state.user.id, conversationId: conversation.id, peers: keyPackages,
        });
      } else {
        if (!conversation.peer_id) throw new Error(translate("Encryption could not be enabled for this conversation."));
        const claimed = await request(`/v1/users/${encodeURIComponent(conversation.peer_id)}/crypto/key-package`, {
          method: "POST", body: JSON.stringify({ client_claim_id: crypto.randomUUID() }),
        });
        const inspected = await invoke("crypto_inspect_key_package", {
          peerUserId: conversation.peer_id, peerDeviceId: claimed.device_id, payload: claimed.payload,
        });
        const approved = confirm(`${translate("Compare this security code with your friend through another trusted channel before continuing. Continue only if the codes match.")}\n\n${inspected.identityFingerprint}`);
        if (!approved) {
          toast(translate("Your friend needs to prepare an encryption device first."));
          return;
        }
        const directRoster = [state.user.id, conversation.peer_id].sort();
        await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/crypto-lock`, {
          method: "POST", body: JSON.stringify({ member_ids: directRoster }),
        });
        state.cryptoLocked.add(conversation.id);
        state.cryptoSetupComplete.delete(conversation.id);
        state.cryptoCanManage.add(conversation.id);
        group = await invoke("crypto_create_direct_group", {
          userId: state.user.id, conversationId: conversation.id, peerUserId: conversation.peer_id,
          peerDeviceId: claimed.device_id, peerPackagePayload: claimed.payload,
        });
      }
      const outbox = group;
      localStorage.setItem(cryptoBootstrapKey(conversation.id), JSON.stringify(outbox));
      await publishCryptoBootstrap(conversation, outbox);
    } catch (error) {
      toast(error.message || translate("Encryption could not be enabled for this conversation."), true);
    } finally {
      button.disabled = false;
      if (!state.cryptoEnabled.has(conversation.id)) updateCryptoStatus(conversation);
    }
  }
  $("#enableEncryptionButton").onclick = () => void enableEncryption();
  async function decodeMessageRow(message) {
    if (typeof message.content !== "string" || !message.content.startsWith("e2e1:")) return message;
    const messageKey = message.client_message_id || message.id;
    if (state.cryptoMessageDecoding.has(messageKey)) return state.cryptoMessageDecoding.get(messageKey);
    const task = (async () => {
      let content = await invoke("crypto_get_cached_sent_text", { clientMessageId: message.client_message_id });
      if (content == null) {
        const result = await invoke("crypto_process_event", {
          userId: state.user.id, conversationId: message.conversation_id, senderId: message.sender_id,
          clientMessageId: message.client_message_id,
          conversationIsGroup: (state.active?.id === message.conversation_id ? state.active : state.conversations.find(item => item.id === message.conversation_id))?.kind === "group",
          expectedMemberIds: await expectedCryptoMemberIds(message.conversation_id), payload: message.content.slice(5),
        });
        if (result.kind === "application") content = result.content;
        else if (result.kind === "own_message") content = await invoke("crypto_get_cached_sent_text", { clientMessageId: message.client_message_id });
      }
      if (typeof content !== "string") throw new Error(translate("This message could not be decrypted on this device."));
      return { ...message, content, encrypted: true };
    })();
    state.cryptoMessageDecoding.set(messageKey, task);
    try { return await task; }
    finally { state.cryptoMessageDecoding.delete(messageKey); }
  }
  async function showIncomingMessage(message) {
    if (state.active?.id !== message.conversation_id || state.page !== "messages") return;
    try { appendMessage(await decodeMessageRow(message)); }
    catch {
      if (message.content?.startsWith("e2e1:")) appendMessage({ ...message, content: translate("This message could not be decrypted on this device."), encrypted: true });
      else appendMessage(message);
    }
    if (state.active?.id === message.conversation_id) { void refreshMessageReactions(); $("#messages").scrollTop = $("#messages").scrollHeight; }
  }
  async function openConversation(id, refresh = true) {
    sendTyping(false);
    setReplyTarget(null);
    closeMobileSidebar();
    $("#sidebarTitle").textContent = translate("Messages");
    $("#sidebarAction").title = translate("Start a conversation");
    $("#sidebarAction").setAttribute("aria-label", translate("Start a conversation"));
    state.page = "messages"; $("#messagePanel").classList.remove("hidden"); $("#pageContent").classList.add("hidden");
    if (refresh) {
      $$(".rail-button[data-page]").forEach(button => {
        const active = button.dataset.page === "messages";
        button.classList.toggle("active", active);
        if (active) button.setAttribute("aria-current", "page");
        else button.removeAttribute("aria-current");
      });
    }
    const convo = state.conversations.find(item => item.id === id);
    if (!convo) { await refreshConversations(); }
    state.active = state.conversations.find(item => item.id === id) || { id, display_name: "Conversation", username: "" };
    $("#chatKindLabel").textContent = translate(state.active.kind === "group" ? "GROUP CONVERSATION" : "DIRECT MESSAGE");
    $("#chatTitle").textContent = state.active.display_name;
    $("#chatSubtitle").textContent = state.active.kind === "group"
      ? `${state.active.member_count} ${translate("members")} · ${state.active.member_names || translate("GROUP MEMBERS")}`
      : `@${state.active.username} · ${presenceLabel(state.active.peer_id)}`;
    setAvatar($("#chatAvatar"), state.active.display_name, state.active.avatar_data);
    if (state.active.kind === "direct") addPresenceDot($("#chatAvatar"), state.active.peer_id);
    $("#chatAvatar").onclick = () => state.active?.kind === "direct" && showProfilePreview({ id: state.active.peer_id, display_name: state.active.display_name, username: state.active.username, avatar_data: state.active.avatar_data });
    renderSafetyControls();
    renderTypingIndicator();
    renderVoiceControls();
    await refreshCryptoConversation(state.active);
    await loadMessages(false);
  }
  function renderTypingIndicator() {
    const indicator = $("#typingIndicator");
    if (!indicator) return;
    const users = [...(state.typingUsers.get(state.active?.id) || new Map()).values()];
    if (!users.length) { indicator.textContent = ""; indicator.classList.add("hidden"); return; }
    indicator.textContent = users.length === 1
      ? `${users[0].displayName} ${translate("is typing…")}`
      : (tr() ? `${users[0].displayName} ve ${users.length - 1} kişi daha yazıyor…` : `${users[0].displayName} and ${users.length - 1} others are typing…`);
    indicator.classList.remove("hidden");
  }
  function updateTypingIndicator(data) {
    if (data.user_id === state.user?.id || typeof data.conversation_id !== "string") return;
    const conversationUsers = state.typingUsers.get(data.conversation_id) || new Map();
    state.typingUsers.set(data.conversation_id, conversationUsers);
    const key = `${data.conversation_id}:${data.user_id}`;
    clearTimeout(state.typingTimers.get(key));
    state.typingTimers.delete(key);
    if (data.active && typeof data.display_name === "string") {
      conversationUsers.set(data.user_id, { displayName: data.display_name });
      state.typingTimers.set(key, setTimeout(() => {
        conversationUsers.delete(data.user_id);
        state.typingTimers.delete(key);
        if (!conversationUsers.size) state.typingUsers.delete(data.conversation_id);
        if (state.active?.id === data.conversation_id) renderTypingIndicator();
      }, 4500));
    } else {
      conversationUsers.delete(data.user_id);
      if (!conversationUsers.size) state.typingUsers.delete(data.conversation_id);
    }
    if (state.active?.id === data.conversation_id) renderTypingIndicator();
  }
  function sendTyping(active) {
    const conversationId = state.active?.id;
    if (!conversationId) return;
    if (active && !$("#messageInput").value.trim()) active = false;
    if (!active && !state.typingActive) return;
    if (active && state.typingActive && Date.now() - state.typingSentAt < 3500) {
      clearTimeout(state.typingStopTimer);
      state.typingStopTimer = setTimeout(() => sendTyping(false), 1800);
      return;
    }
    if (!state.socket || state.socket.readyState !== WebSocket.OPEN) { state.typingActive = false; return; }
    state.socket.send(JSON.stringify({ type: "typing", conversation_id: conversationId, active }));
    state.typingActive = active;
    state.typingSentAt = Date.now();
    clearTimeout(state.typingStopTimer);
    if (active) state.typingStopTimer = setTimeout(() => sendTyping(false), 1800);
  }
  async function markConversationRead(conversationId, sequence) {
    if (!Number.isSafeInteger(Number(sequence)) || state.page !== "messages" || !document.hasFocus() || document.visibilityState !== "visible") return;
    const parsed = Number(sequence);
    if (parsed <= (state.lastReadSequence.get(conversationId) || 0)) return;
    try {
      const result = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/read`, { method: "POST", body: JSON.stringify({ sequence: parsed }) });
      state.lastReadSequence.set(conversationId, result.last_read_sequence || parsed);
      const conversation = state.conversations.find(item => item.id === conversationId);
      if (conversation) conversation.unread_count = 0;
      renderSidebar();
    } catch (error) { console.warn("Could not update read position", error); }
  }
  function timeLabel(value) {
    const date = new Date(value); if (Number.isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(date);
  }
  function appendMessageText(node, content) {
    const urlPattern = /https?:\/\/[^\s<>"']+/gi;
    let cursor = 0;
    for (const match of content.matchAll(urlPattern)) {
      const start = match.index ?? 0;
      if (start > cursor) node.append(document.createTextNode(content.slice(cursor, start)));
      const rawUrl = match[0];
      const punctuation = rawUrl.match(/[.,!?;:]+$/)?.[0] || "";
      const candidate = rawUrl.slice(0, rawUrl.length - punctuation.length);
      try {
        const parsed = new URL(candidate);
        if (parsed.protocol !== "https:" && parsed.protocol !== "http:") throw new Error("Unsupported link protocol");
        const link = create("button", "message-link", candidate);
        link.type = "button";
        link.title = candidate;
        link.setAttribute("aria-label", `${translate("Open link")}: ${candidate}`);
        link.onclick = () => openExternalLink(parsed.href);
        node.append(link);
      } catch {
        node.append(document.createTextNode(candidate));
      }
      if (punctuation) node.append(document.createTextNode(punctuation));
      cursor = start + rawUrl.length;
    }
    if (cursor < content.length) node.append(document.createTextNode(content.slice(cursor)));
  }
  function openExternalLink(url) {
    let parsed;
    try {
      parsed = new URL(url);
      if (parsed.protocol !== "https:" && parsed.protocol !== "http:") throw new Error("Unsupported link protocol");
    } catch {
      toast(translate("This link is invalid."), true);
      return;
    }
    if (!confirm(`${translate("Open external link?")}\n${parsed.href}`)) return;
    const anchor = document.createElement("a");
    anchor.href = parsed.href;
    anchor.target = "_blank";
    anchor.rel = "noopener noreferrer";
    anchor.style.display = "none";
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
  }
  function encryptedAttachmentManifest(content) {
    if (typeof content !== "string" || !content.startsWith(ENCRYPTED_ATTACHMENT_MARKER)) return null;
    try {
      const item = JSON.parse(content.slice(ENCRYPTED_ATTACHMENT_MARKER.length));
      const sizeBytes = Number(item.sizeBytes); const chunkSize = Number(item.chunkSize); const chunkCount = Number(item.chunkCount);
      if (!/^[0-9a-f-]{36}$/i.test(item.attachmentId || "") || !Number.isSafeInteger(sizeBytes) || sizeBytes < 0 || sizeBytes > 5 * 1024 * 1024 * 1024 || chunkSize !== 1024 * 1024 || chunkCount !== Math.max(1, Math.ceil(sizeBytes / chunkSize))) return null;
      if (typeof item.fileName !== "string" || typeof item.contentType !== "string" || typeof item.key !== "string" || typeof item.noncePrefix !== "string") return null;
      if (item.key.length > 64 || item.noncePrefix.length > 24 || item.fileName.length > 120 || item.contentType.length > 120) return null;
      return { ...item, sizeBytes, chunkSize, chunkCount };
    } catch { return null; }
  }
  function appendMessage(message, prepend = false) {
    const container = $("#messages"); if ($(`[data-message-id="${CSS.escape(message.id)}"]`, container)) return;
    const ownMessage = message.sender_id === state.user.id;
    const row = create("article", `message ${ownMessage ? "own-message" : "peer-message"}`); row.dataset.messageId = message.id; row.dataset.sequence = String(message.sequence);
    const avatar = create("div", "mini-avatar message-avatar"); setAvatar(avatar, message.sender_name, message.sender_avatar); avatar.title = message.sender_name; avatar.setAttribute("aria-label", message.sender_name); row.append(avatar);
    const main = create("div", "message-main"); const meta = create("div", "message-meta"); meta.append(create("strong", "", message.sender_name)); if (ownMessage) meta.append(create("span", "message-owner-label", translate("You"))); if (message.encrypted) meta.append(create("span", "message-encrypted-badge", "🔒")); if (message.pinned_at) meta.append(create("span", "message-pinned-badge", `⌖ ${translate("Pinned")}`)); const time = create("time", "", timeLabel(message.created_at)); time.dateTime = message.created_at; meta.append(time); if (message.edited_at) meta.append(create("span", "optional", "edited"));
    main.append(meta);
    if (message.reply_to_id) {
      const quote = create("button", "reply-reference"); quote.type = "button";
      const quoteName = message.reply_to_sender || translate("Reply");
      const quoteContent = (message.reply_to_content || "").startsWith("file:") ? translate("File shared") : (message.reply_to_content || "").startsWith("e2e1:") ? translate("Encrypted message") : (message.reply_to_content || "");
      quote.append(create("strong", "", quoteName), create("span", "", quoteContent.slice(0, 160)));
      quote.onclick = () => { const target = $(`[data-message-id="${CSS.escape(message.reply_to_id)}"]`, container); if (target) { target.scrollIntoView({ behavior: "smooth", block: "center" }); target.classList.add("message-highlight"); setTimeout(() => target.classList.remove("message-highlight"), 1000); } };
      main.append(quote);
    }
    const encryptedAttachment = message.encrypted ? encryptedAttachmentManifest(message.content) : null;
    if (encryptedAttachment) {
      renderEncryptedAttachment(main, encryptedAttachment);
    } else if (message.encrypted && message.content.startsWith(ENCRYPTED_ATTACHMENT_MARKER)) {
      main.append(create("div", "message-body", translate("This encrypted attachment could not be opened.")));
    } else if (message.content.startsWith("file:")) {
      const attachment = create("div", "message-attachment");
      main.append(attachment);
      if ("IntersectionObserver" in window) {
        state.attachmentObserver ||= new IntersectionObserver(entries => {
          for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            state.attachmentObserver.unobserve(entry.target);
            void renderAttachment(entry.target, entry.target.dataset.attachmentId, entry.target.closest(".message"));
          }
        }, { root: container, rootMargin: "120px" });
        attachment.dataset.attachmentId = message.content.slice("file:".length);
        state.attachmentObserver.observe(attachment);
      } else void renderAttachment(attachment, message.content.slice("file:".length), row);
    } else if (message.content) {
      const body = create("div", "message-body");
      appendMessageText(body, message.content);
      main.append(body);
    }
    if (!message.encrypted) {
      const reactions = create("div", "message-reactions"); reactions.setAttribute("aria-label", translate("Message reactions")); main.append(reactions);
      const quickReactions = create("div", "quick-reactions"); reactions.append(quickReactions);
      const inlineActions = create("div", "message-inline-actions");
      const addReaction = create("button", "message-inline-action add-reaction", "+"); addReaction.type = "button"; addReaction.title = translate("Add a reaction"); addReaction.setAttribute("aria-label", addReaction.title); addReaction.onclick = async () => {
        const emoji = prompt(translate("Add a reaction"), "");
        if (!emoji?.trim()) return;
        addReaction.disabled = true;
        try { await request(`/v1/messages/${encodeURIComponent(message.id)}/reactions`, { method: "POST", body: JSON.stringify({ emoji: emoji.trim().slice(0, 16) }) }); await refreshMessageReactions(); }
        catch (error) { toast(error.message, true); }
        finally { addReaction.disabled = false; }
      };
      const reply = create("button", "message-inline-action", translate("Reply")); reply.type = "button"; reply.onclick = () => setReplyTarget(message);
      const pin = create("button", "message-inline-action", message.pinned_at ? translate("Unpin message") : translate("Pin message")); pin.type = "button"; pin.onclick = async () => { pin.disabled = true; try { await request(`/v1/messages/${encodeURIComponent(message.id)}/pin`, { method: "POST" }); await loadMessages(false); } catch (error) { toast(error.message, true); pin.disabled = false; } };
      inlineActions.append(addReaction, reply, pin); reactions.append(inlineActions);
    }
    row.append(main);
    if (ownMessage) {
      const actions = create("div", "message-actions"); if (!message.encrypted) { const edit = create("button", "", "Edit"); edit.onclick = () => editMessage(message); actions.append(edit); } const remove = create("button", "", "Delete"); remove.onclick = () => deleteMessage(message.id); actions.append(remove); row.append(actions);
    }
    if (prepend) container.prepend(row); else container.append(row);
    updateSeenBadge();
  }
  function updateSeenBadge() {
    if (!state.active || state.active.kind !== "direct") return;
    const conversation = state.conversations.find(item => item.id === state.active.id);
    const rows = $$(".message.own-message", $("#messages"));
    for (const row of rows) $(".message-seen-badge", row)?.remove();
    const lastOwn = rows.at(-1);
    if (lastOwn && Number(conversation?.peer_read_sequence || 0) >= Number(lastOwn.dataset.sequence || 0)) {
      $(".message-meta", lastOwn)?.append(create("span", "message-seen-badge", translate("Seen")));
    }
  }
  function setReplyTarget(message) {
    state.replyingTo = message || null;
    const preview = $("#replyComposerPreview");
    preview.classList.toggle("hidden", !message);
    if (message) {
      const content = message.content.startsWith("file:") ? translate("File shared") : message.content;
      $("#replyComposerText").textContent = `${translate("Replying to")} ${message.sender_name}: ${content.slice(0, 140)}`;
      $("#messageInput").focus();
    }
  }
  async function refreshMessageReactions() {
    if (!state.active) return;
    const conversationId = state.active.id;
    try {
      const summaries = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/reactions`);
      if (state.active?.id !== conversationId) return;
      const grouped = new Map();
      for (const item of summaries) { if (!grouped.has(item.message_id)) grouped.set(item.message_id, new Map()); grouped.get(item.message_id).set(item.emoji, item); }
      for (const row of $$(".message[data-message-id]", $("#messages"))) {
        const quick = $(".quick-reactions", row); if (!quick) continue;
        quick.replaceChildren();
        const messageId = row.dataset.messageId;
        for (const [emoji, item] of grouped.get(messageId) || []) {
          const button = create("button", `reaction-pill${item.reacted ? " active" : ""}`, `${emoji} ${item.count}`);
          button.type = "button"; button.title = `${translate("Add a reaction")} ${emoji}`; button.setAttribute("aria-label", button.title); button.setAttribute("aria-pressed", String(!!item.reacted));
          button.onclick = async () => {
            button.disabled = true;
            try { await request(`/v1/messages/${encodeURIComponent(messageId)}/reactions`, { method: "POST", body: JSON.stringify({ emoji }) }); await refreshMessageReactions(); }
            catch (error) { toast(error.message, true); button.disabled = false; }
          };
          quick.append(button);
        }
      }
    } catch (error) { console.warn("Could not refresh message reactions", error); }
  }
  async function loadMessages(older = false) {
    if (!state.active) return;
    const conversationId = state.active.id;
    const container = $("#messages"); const oldHeight = container.scrollHeight; const first = container.firstElementChild;
    let path = `/v1/conversations/${encodeURIComponent(conversationId)}/messages?limit=50`;
    if (older && first) path += `&before=${encodeURIComponent(first.dataset.sequence || "")}`;
    try {
      if (!older && state.cryptoEnabled.has(conversationId) && !state.cryptoMembershipPending.has(conversationId)) await syncCryptoEvents(conversationId);
      const rows = await request(path); if (state.active?.id !== conversationId) return; if (!older) { clearAttachmentUrls(); container.replaceChildren(); }
      for (const row of rows) {
        try { appendMessage(await decodeMessageRow(row), older); }
        catch (error) { if (row.content?.startsWith("e2e1:")) toast(error.message || translate("This message could not be decrypted on this device."), true); else appendMessage(row, older); }
      }
      await refreshMessageReactions();
      state.older = rows.length === 50; $("#loadOlderRow").classList.toggle("hidden", !state.older || !rows.length);
      if (older) container.scrollTop += container.scrollHeight - oldHeight; else container.scrollTop = container.scrollHeight;
      if (!older && rows.length) await markConversationRead(conversationId, Number(rows.at(-1).sequence));
      await refreshConversations(); updateSeenBadge();
    } catch (error) { toast(error.message, true); }
  }
  function encryptedMessagePendingKey(conversationId) { return `ghostchat.cryptoMessagePending.${state.base}.${state.user?.id || ""}.${conversationId}`; }
  async function retryPendingEncryptedMessage(conversation) {
    const key = encryptedMessagePendingKey(conversation.id); const raw = localStorage.getItem(key);
    if (!raw) return true;
    const pending = JSON.parse(raw);
    const sent = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/messages`, {
      method: "POST", body: JSON.stringify({ client_message_id: pending.clientMessageId, content: `e2e1:${pending.payload}`, reply_to_id: null }),
    });
    localStorage.removeItem(key);
    const plaintext = await invoke("crypto_get_cached_sent_text", { clientMessageId: pending.clientMessageId });
    if (state.active?.id === conversation.id && plaintext) appendMessage({ ...sent, content: plaintext, encrypted: true });
    return true;
  }
  async function sendMessage(content, clientId = crypto.randomUUID(), replyToId = null, targetConversation = state.active) {
    if (!targetConversation || !content.trim()) return;
    if (state.cryptoMembershipPending.has(targetConversation.id)) {
      toast(translate("Pending encrypted membership update is being retried…"), true);
      return false;
    }
    if (state.cryptoLocked.has(targetConversation.id) && !state.cryptoEnabled.has(targetConversation.id)) {
      toast(translate("This conversation is locked while MLS encryption is being set up. Finish setup before sending messages."), true);
      return false;
    }
    if (state.cryptoEnabled.has(targetConversation.id)) {
      if (!cryptoIdentityTrusted(targetConversation.id)) {
        if (state.active?.id === targetConversation.id) updateCryptoStatus(targetConversation);
        toast(translate(targetConversation.kind === "group"
          ? "The encrypted group identity is not verified. Compare the full group code with its members before sending private messages."
          : "The encrypted identity is not verified. Compare this code with your friend before sending private messages."), true);
        return;
      }
      try {
        const conversation = targetConversation;
        if (!await retryPendingEncryptedMessage(conversation)) return;
        const encrypted = await invoke("crypto_encrypt_text", {
          userId: state.user.id, conversationId: conversation.id, clientMessageId: clientId, content,
        });
        localStorage.setItem(encryptedMessagePendingKey(conversation.id), JSON.stringify({ clientMessageId: clientId, payload: encrypted.payload }));
        const message = await request(`/v1/conversations/${encodeURIComponent(conversation.id)}/messages`, {
          method: "POST", body: JSON.stringify({ client_message_id: clientId, content: `e2e1:${encrypted.payload}`, reply_to_id: null }),
        });
        localStorage.removeItem(encryptedMessagePendingKey(conversation.id));
        if (state.active?.id === conversation.id) {
          appendMessage({ ...message, content, encrypted: true });
          $("#messages").scrollTop = $("#messages").scrollHeight;
        }
        await refreshConversations(); updateSeenBadge(); return true;
      } catch (error) {
        toast(error.message || translate("The encrypted message is waiting to be sent. Try again when connected."), true);
        return false;
      }
    }
    try { const message = await request(`/v1/conversations/${encodeURIComponent(targetConversation.id)}/messages`, { method: "POST", body: JSON.stringify({ client_message_id: clientId, content, reply_to_id: replyToId }) }); if (state.active?.id === targetConversation.id) { appendMessage(message); await refreshMessageReactions(); $("#messages").scrollTop = $("#messages").scrollHeight; } await refreshConversations(); updateSeenBadge(); return true; }
    catch (error) { toast(error.message, true); }
  }
  async function editMessage(message) {
    const content = prompt("Edit message", message.content); if (content === null || content.trim() === message.content) return;
    try { await request(`/v1/messages/${encodeURIComponent(message.id)}`, { method: "PATCH", body: JSON.stringify({ content }) }); await loadMessages(false); }
    catch (error) { toast(error.message, true); }
  }
  async function deleteMessage(id) {
    if (!confirm("Delete this message?")) return;
    try { await request(`/v1/messages/${encodeURIComponent(id)}`, { method: "DELETE" }); $(`[data-message-id="${CSS.escape(id)}"]`)?.remove(); await refreshConversations(); }
    catch (error) { toast(error.message, true); }
  }
  function clearAttachmentUrls() {
    state.attachmentObserver?.disconnect();
    state.attachmentObserver = null;
    for (const url of state.attachmentObjectUrls) URL.revokeObjectURL(url);
    state.attachmentObjectUrls = [];
  }
  async function renderAttachment(container, attachmentId, messageRow) {
    try {
      const base = activeBase();
      const attachment = await request(`/v1/attachments/${encodeURIComponent(attachmentId)}/link`, { method: "POST" });
      if (!container.isConnected || !messageRow?.isConnected) return;
      const url = new URL(attachment.url, base).toString();
      const filename = attachment.filename || "Shared file";
      if (attachment.content_type?.startsWith("image/")) {
        container.classList.add("image-attachment");
        const loading = create("span", "attachment-loading", translate("Loading image…")); container.append(loading);
        let previewUrl = url;
        try {
          const downloadUrl = new URL(`/v1/attachments/${encodeURIComponent(attachmentId)}`, activeBase());
          const response = await fetch(downloadUrl, { headers: { Authorization: `Bearer ${state.token}` } });
          if (!response.ok) throw new Error("Image download failed");
          const blob = await response.blob();
          previewUrl = URL.createObjectURL(blob);
          state.attachmentObjectUrls.push(previewUrl);
        } catch (error) { console.warn("Using attachment preview link", error); }
        if (!container.isConnected || !messageRow?.isConnected) return;
        const image = create("img", "message-image"); image.alt = filename; image.loading = "lazy";
        image.onload = () => loading.remove();
        image.onerror = () => {
          loading.remove(); image.remove();
          const fallback = create("a", "attachment-download", `⇩  ${filename}`); fallback.href = url; fallback.download = filename; container.prepend(fallback);
        };
        image.src = previewUrl;
        container.append(image);
        container.append(create("span", "attachment-filename", filename));
      } else if (attachment.content_type?.startsWith("video/")) {
        const video = create("video", "message-video"); video.src = url; video.controls = true; video.preload = "metadata"; container.append(video);
        container.append(create("span", "attachment-filename", filename));
      } else {
        const link = create("a", "attachment-download", `⇩  ${filename}`); link.href = url; link.download = filename; container.append(link);
      }
    } catch { container.append(create("span", "attachment-download unavailable", translate("Could not load this file."))); }
  }
  function decodeBase64Url(value) {
    const normalized = value.replace(/-/g, "+").replace(/_/g, "/");
    const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "="));
    return Uint8Array.from(binary, character => character.charCodeAt(0));
  }
  function formatFileSize(value) {
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
    if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
    return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
  function renderEncryptedAttachment(main, manifest) {
    const card = create("div", "message-attachment encrypted-attachment");
    card.dataset.attachmentId = manifest.attachmentId;
    card.append(create("span", "attachment-filename", `🔒 ${manifest.fileName}`));
    card.append(create("span", "attachment-size", formatFileSize(manifest.sizeBytes)));
    const actions = create("div", "encrypted-attachment-actions");
    const save = create("button", "attachment-download", translate("Save decrypted file")); save.type = "button";
    save.onclick = async () => {
      save.disabled = true; save.textContent = translate("Downloading and decrypting securely…");
      try {
        const path = await invoke("crypto_download_encrypted_attachment", {
          serverBase: activeBase(), token: state.token, manifest,
        });
        if (path) toast(`${translate("Encrypted file saved.")} ${path}`);
      } catch (error) { toast(error.message || translate("Could not load this file."), true); }
      finally { save.disabled = false; save.textContent = translate("Save decrypted file"); }
    };
    actions.append(save);
    const previewable = manifest.sizeBytes <= 32 * 1024 * 1024
      && (["image/avif", "image/gif", "image/jpeg", "image/png", "image/webp", "video/mp4", "video/webm", "video/quicktime"].includes(manifest.contentType));
    if (previewable) {
      const preview = create("button", "attachment-preview", translate("Preview securely")); preview.type = "button";
      preview.onclick = async () => {
        preview.disabled = true;
        try {
          const blob = await decryptAttachmentPreview(manifest);
          const objectUrl = URL.createObjectURL(blob); state.attachmentObjectUrls.push(objectUrl);
          if (manifest.contentType.startsWith("image/")) {
            const image = create("img", "message-image"); image.src = objectUrl; image.alt = manifest.fileName; image.loading = "lazy"; card.insertBefore(image, actions);
          } else {
            const video = create("video", "message-video"); video.src = objectUrl; video.controls = true; video.preload = "metadata"; card.insertBefore(video, actions);
          }
          preview.remove();
        } catch (error) { toast(error.message || translate("The encrypted file could not be previewed."), true); preview.disabled = false; }
      };
      actions.append(preview);
    }
    card.append(actions); main.append(card);
  }
  async function decryptAttachmentPreview(manifest) {
    const url = new URL(`/v1/attachments/${encodeURIComponent(manifest.attachmentId)}`, activeBase());
    const response = await fetch(url, { headers: { Authorization: `Bearer ${state.token}` } });
    if (!response.ok) throw new Error(translate("The encrypted file could not be previewed."));
    const encrypted = new Uint8Array(await response.arrayBuffer());
    const expectedSize = manifest.sizeBytes + manifest.chunkCount * 16;
    if (encrypted.byteLength !== expectedSize) throw new Error(translate("The encrypted file could not be previewed."));
    const keyBytes = decodeBase64Url(manifest.key); const noncePrefix = decodeBase64Url(manifest.noncePrefix);
    if (keyBytes.byteLength !== 32 || noncePrefix.byteLength !== 8) throw new Error(translate("The encrypted file could not be previewed."));
    const key = await crypto.subtle.importKey("raw", keyBytes, { name: "AES-GCM" }, false, ["decrypt"]);
    const aadPrefix = new TextEncoder(); const chunks = [];
    let encryptedOffset = 0;
    for (let index = 0; index < manifest.chunkCount; index++) {
      const plainSize = Math.min(manifest.chunkSize, manifest.sizeBytes - index * manifest.chunkSize);
      const encryptedSize = plainSize + 16;
      const nonce = new Uint8Array(12); nonce.set(noncePrefix); new DataView(nonce.buffer).setUint32(8, index, false);
      const aad = aadPrefix.encode("ghostchat-file-v1:" + manifest.attachmentId + ":" + index);
      const decrypted = await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 }, key, encrypted.subarray(encryptedOffset, encryptedOffset + encryptedSize));
      chunks.push(decrypted); encryptedOffset += encryptedSize;
    }
    return new Blob(chunks, { type: manifest.contentType });
  }
  async function uploadAttachment(file) {
    if (!state.active || !file) return;
    if (state.cryptoEnabled.has(state.active.id) || state.cryptoLocked.has(state.active.id)) { toast(translate("File sharing is not encrypted in this conversation yet."), true); return; }
    if (file.size > 5 * 1024 * 1024 * 1024) { toast(translate("File is too large. The limit is 5 GB."), true); return; }
    const button = $("#attachFileButton"); button.disabled = true;
    const messageId = crypto.randomUUID();
    const conversationId = state.active.id;
    const base = activeBase();
    const query = `name=${encodeURIComponent(file.name)}&client_message_id=${encodeURIComponent(messageId)}`;
    try {
      const response = await fetch(`${base}/v1/conversations/${encodeURIComponent(conversationId)}/attachments?${query}`, {
        method: "POST",
        headers: { Authorization: `Bearer ${state.token}`, "Content-Type": file.type || "application/octet-stream" },
        body: file,
      });
      const body = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(body.error || `Upload failed (${response.status}).`);
      if (state.active?.id === conversationId) { appendMessage(body); $("#messages").scrollTop = $("#messages").scrollHeight; }
      await refreshConversations(); toast(translate("File sent."));
    } catch (error) { toast(error.message || "Could not reach the selected server. Check its address and connection.", true); }
    finally { button.disabled = false; $("#attachmentInput").value = ""; }
  }
  async function uploadEncryptedAttachment() {
    const conversation = state.active;
    if (!conversation || !state.cryptoEnabled.has(conversation.id)) return;
    if (!cryptoIdentityTrusted(conversation.id)) {
      updateCryptoStatus(conversation);
      toast(translate("The encrypted identity is not verified. Compare this code with your friend before sending private messages."), true);
      return;
    }
    const button = $("#attachFileButton");
    const conversationId = conversation.id;
    button.disabled = true;
    button.title = translate("Encrypting and uploading securely…");
    try {
      const manifest = await invoke("crypto_upload_encrypted_attachment", {
        conversationId, serverBase: activeBase(), token: state.token,
      });
      if (!manifest) return;
      const content = ENCRYPTED_ATTACHMENT_MARKER + JSON.stringify(manifest);
      if (await sendMessage(content, crypto.randomUUID(), null, conversation)) toast(translate("Secure file sent."));
    } catch (error) {
      toast(translate(error.message) || error.message || translate("Could not load this file."), true);
    } finally {
      button.disabled = false;
      button.title = button.getAttribute(tr() ? "data-tr-title" : "data-en-title") || translate("Attach a photo, video, or file");
    }
  }
  function setConnection(online) {
    const node = $("#connectionState"); node.classList.toggle("offline", !online); node.innerHTML = `<i class="status-dot ${online ? "" : "offline"}"></i>${translate(online ? "Connected" : "Reconnecting")}`;
  }
  function connectSocket() {
    if (state.socket) { try { state.socket.close(); } catch {} }
    const url = activeBase().replace(/^http:/, "ws:").replace(/^https:/, "wss:");
    try { state.socket = new WebSocket(`${url}/v1/ws`); } catch { setConnection(false); scheduleReconnect(); return; }
    const socket = state.socket;
    socket.addEventListener("open", () => { if (socket === state.socket) socket.send(JSON.stringify({ token: state.token })); });
    socket.addEventListener("message", event => {
      if (socket !== state.socket) return;
      let data; try { data = JSON.parse(event.data); } catch { return; }
      if (data.type === "ready") { state.reconnect = 1200; setConnection(true); void refreshPresence(); void refreshFriends(); refreshConversations(); if (state.active) loadMessages(false); }
      else if (data.type === "session_revoked") { voiceClient?.hangup(); void groupVoiceClient?.leave(); state.token = ""; state.user = null; state.blockedUserIds.clear(); state.cryptoDevice = null; state.active = null; resetCryptoViewState(); if (state.socket) state.socket.close(); void (async () => { try { await invoke("credential_delete", { serverKey: await serverKey(activeBase()) }); } catch {} })(); show("authView"); toast("This session was revoked. Recover your account to sign in again.", true); }
      else if (data.type === "crypto_event" && data.event?.conversation_id) {
        const conversationId = data.event.conversation_id;
        if (state.active?.id === conversationId) void refreshCryptoConversation(state.active);
        else if (state.cryptoEnabled.has(conversationId)) void syncCryptoEvents(conversationId);
      }
      else if (data.type === "message" && data.message) {
        const message = data.message;
        const activeAndVisible = state.active?.id === message.conversation_id && state.page === "messages" && document.hasFocus() && document.visibilityState === "visible";
        if (state.active?.id === message.conversation_id) void showIncomingMessage(message);
        if (message.sender_id !== state.user?.id && (state.active?.id !== message.conversation_id || !document.hasFocus())) {
          void sendDesktopNotification(`Message from ${message.sender_name}`, "You received a new message in GhostChat.");
        }
        if (activeAndVisible) void markConversationRead(message.conversation_id, Number(message.sequence)).then(() => refreshConversations().then(updateSeenBadge));
        else void refreshConversations().then(updateSeenBadge);
      }
      else if (data.type === "friend_request") { void refreshFriends(); }
      else if (data.type === "user_block_updated") { if (state.active?.kind === "direct" && state.active.peer_id === data.user_id) voiceClient?.hangup(); void refreshBlockedUsers(); void refreshFriends({ notifyNew: false }); void refreshConversations(); }
      else if (data.type === "contacts_updated") { void refreshFriends({ notifyNew: false }); void refreshConversations(); }
      else if (data.type === "conversation_created") { void refreshConversations(); if (state.page === "messages" && data.created_by !== state.user?.id) toast(translate("You were added to a group.")); }
      else if (data.type === "reaction_updated") {
        if (state.active?.id === data.conversation_id) void refreshMessageReactions();
      }
      else if (data.type === "message_pin_updated") {
        if (state.active?.id === data.conversation_id) void loadMessages(false);
      }
      else if (data.type === "typing") updateTypingIndicator(data);
      else if (data.type === "read_receipt") void refreshConversations().then(() => { if (state.active?.id === data.conversation_id) updateSeenBadge(); });
      else if (data.type === "conversation_updated") {
        const previousConversation = state.conversations.find(item => item.id === data.conversation_id);
        if (groupVoiceUiState.status !== "idle" && data.conversation_id === groupVoiceUiState.conversationId) {
          void (async () => {
            try {
              const members = await request(`/v1/conversations/${encodeURIComponent(data.conversation_id)}/members`);
              if (!members.some(member => member.id === state.user?.id)) await groupVoiceClient.leave();
            } catch (error) {
              if (error.status === 404) await groupVoiceClient.leave();
            }
          })();
        }
        void refreshConversations().then(async () => {
          if (state.active?.id !== data.conversation_id) return;
          const active = state.conversations.find(item => item.id === data.conversation_id);
          if (!active) {
            if (previousConversation?.kind === "group" && state.cryptoLocked.has(data.conversation_id)) {
              try { await invoke("crypto_forget_group_state", { userId: state.user.id, conversationId: data.conversation_id }); }
              catch (error) { console.warn("Could not clear the removed local MLS group state.", error); }
              state.cryptoEnabled.delete(data.conversation_id);
              state.cryptoLocked.delete(data.conversation_id);
              state.cryptoSetupComplete.delete(data.conversation_id);
              state.cryptoCanManage.delete(data.conversation_id);
              state.cryptoPeerFingerprints.delete(data.conversation_id);
              state.cryptoMemberIds.delete(data.conversation_id);
              state.cryptoMembershipPending.delete(data.conversation_id);
              state.cryptoSyncing.delete(data.conversation_id);
              localStorage.removeItem(cryptoObservedFingerprintKey(data.conversation_id));
              localStorage.removeItem(cryptoTrustedFingerprintKey(data.conversation_id));
              localStorage.removeItem(cryptoEventCursorKey(data.conversation_id));
            }
            state.active = null; $("#groupManageDialog").close(); showWelcome();
          } else {
            await openConversation(active.id, false);
            if ($("#groupManageDialog").open) await refreshGroupManageDialog();
          }
        });
      }
      else if (data.type === "profile_updated") {
        void refreshFriends({ notifyNew: false });
        void refreshConversations().then(() => { if (state.active) void openConversation(state.active.id, false); });
      }
      else if (data.type === "presence_updated" && data.user_id) {
        state.presence.set(data.user_id, data.mode || "online");
        if (data.user_id === state.user?.id) state.presenceMode = data.mode || "online";
        updateIdentity(); renderSidebar();
        if (state.active?.kind === "direct" && state.active.peer_id === data.user_id) void openConversation(state.active.id, false);
      }
      else if (data.type === "friend_request_accepted") {
        void refreshFriends();
        if (data.by_user_id !== state.user?.id) {
          toast("Your friend request was accepted.");
          void sendDesktopNotification("Friend request accepted", "You can now chat with your new friend.");
        }
      }
      else if (data.type === "voice_signal") { void voiceClient?.handleSignal(data); }
      else if (data.type === "deleted" || data.type === "refresh") { if (state.active) loadMessages(false); }
    });
    socket.addEventListener("close", () => { if (socket === state.socket) { state.typingActive = false; for (const timer of state.typingTimers.values()) clearTimeout(timer); state.typingTimers.clear(); state.typingUsers.clear(); renderTypingIndicator(); setConnection(false); scheduleReconnect(); } });
    socket.addEventListener("error", () => { try { socket.close(); } catch {} });
  }
  function scheduleReconnect() {
    clearTimeout(connectSocket.timer); connectSocket.timer = setTimeout(() => { if (state.token) connectSocket(); }, state.reconnect);
    state.reconnect = Math.min(30000, state.reconnect * 1.7);
  }
  function renderVoiceControls() {
    $("#manageGroupButton").classList.toggle("hidden", !state.active || state.active.kind !== "group");
    if (!voiceClient) return;
    const directVoice = voiceClient.getState();
    const groupVoice = groupVoiceClient?.getState() || groupVoiceUiState;
    voiceUiState = directVoice;
    groupVoiceUiState = groupVoice;
    const groupMode = groupVoice.status !== "idle" || (state.active?.kind === "group" && directVoice.status === "idle");
    const voice = groupMode ? groupVoice : directVoice;
    const activeCall = !["idle", "incoming"].includes(voice.status);
    const visible = !!state.active || directVoice.status !== "idle" || groupVoice.status !== "idle";
    $("#voiceTools").classList.toggle("hidden", !visible);
    const blockedDirect = state.active?.kind === "direct" && state.blockedUserIds.has(state.active.peer_id);
    const canStart = !!state.active && !blockedDirect && directVoice.status === "idle" && groupVoice.status === "idle";
    $("#voiceCallButton").classList.toggle("hidden", !canStart);
    const groupNotReady = state.active?.kind === "group" && state.groupVoiceAvailable === false;
    $("#voiceCallButton").disabled = !canStart || groupNotReady;
    const callActionLabel = state.active?.kind === "group" ? translate("Join voice") : translate("Call");
    $("#voiceCallButton").querySelector("span:last-child").textContent = callActionLabel;
    $("#voiceCallButton").title = groupNotReady ? translate("Group voice is not configured on this server. Configure LiveKit in the server environment.") : state.active?.kind === "group" ? translate("Join group voice") : translate("Start voice call");
    $("#voiceCallButton").setAttribute("aria-label", callActionLabel);
    const labels = { idle: groupMode ? (state.groupVoiceAvailable === false ? "Group voice not configured" : "Group voice ready") : "Voice ready", incoming: "Incoming call", calling: "Calling…", connecting: "Connecting…", connected: groupMode ? "Group voice connected" : "In voice", reconnecting: "Reconnecting…" };
    $("#voiceStatus").textContent = translate(labels[voice.status] || "Voice");
    const voiceConversationId = groupMode ? groupVoice.conversationId : directVoice.conversationId;
    const conversation = state.conversations.find(item => item.id === voiceConversationId) || (state.active?.id === voiceConversationId ? state.active : null);
    const peer = groupMode ? (conversation?.display_name || translate("Group call")) : (conversation?.display_name || "Your friend");
    $("#voicePeerName").textContent = peer;
    setAvatar($("#voicePeerAvatar"), peer, conversation?.avatar_data);
    const localName = state.user?.display_name || translate("You");
    $("#voiceLocalName").textContent = localName;
    setAvatar($("#voiceLocalAvatar"), localName, state.user?.avatar_data);
    $("#voicePeerCard").classList.toggle("hidden", groupMode);
    $("#voiceLocalCard").classList.toggle("hidden", groupMode);
    $("#groupVoiceParticipants").classList.toggle("hidden", !groupMode || voice.status === "idle");
    if (groupMode) renderGroupVoiceParticipants(groupVoice.participants || []);
    const pushToTalk = groupMode ? voiceSettings().pushToTalk : voice.pushToTalk;
    const localMuted = !voice.micAvailable || (voice.micMuted && (!pushToTalk || voice.micManuallyMuted || !voice.pushToTalkPressed));
    $("#voiceLocalMutedBadge").classList.toggle("hidden", !localMuted);
    $("#voiceLocalMutedBadge").textContent = translate(!voice.micAvailable ? "No microphone" : voice.pushToTalk && !voice.micManuallyMuted ? "Hold Space" : "Mic off");
    $("#voiceLocalSpeaking").textContent = translate(!voice.micAvailable ? "No microphone" : localMuted ? (voice.pushToTalk && !voice.micManuallyMuted ? "Hold Space" : "Mic off") : "Microphone on");
    $("#voicePeerSpeaking").textContent = translate(isPeerAudioMuted() ? "Muted for you" : "Listening");
    if (!groupMode) renderPeerMuteState();
    $("#voiceStageStatus").textContent = translate(voice.status === "incoming" ? "Incoming call" : (labels[voice.status] || "Voice call"));
    $("#voiceStageDot").classList.toggle("connecting", !["connected", "incoming"].includes(voice.status));
    $("#voiceStageDot").classList.toggle("offline", voice.status === "reconnecting");
    $("#voiceDeviceRow").classList.toggle("hidden", !activeCall);
    $("#voiceStageMicButton").classList.toggle("hidden", !activeCall);
    $("#voiceStageMicButton").classList.toggle("is-muted", voice.micMuted && (!pushToTalk || voice.micManuallyMuted || !voice.micAvailable));
    $("#voiceStageMicButton").setAttribute("aria-pressed", String(voice.micMuted));
    $("#voiceStageMicIcon").textContent = voice.micMuted ? "🎙" : "🎤";
    $("#voiceStageMicLabel").textContent = translate(!voice.micAvailable ? "Try microphone" : pushToTalk ? (voice.micManuallyMuted ? "Mic off" : (voice.pushToTalkPressed ? "Speaking" : "Hold Space")) : (voice.micMuted ? "Turn mic on" : "Mute mic"));
    $("#voiceStageMicButton").title = translate(!voice.micAvailable ? "Try to connect a microphone; if none is available you can keep listening muted" : (voice.micMuted ? "Turn microphone on" : "Mute microphone"));
    $("#voiceOutputButton").classList.toggle("hidden", !activeCall);
    $("#voiceOutputButton").classList.toggle("is-muted", voice.speakerMuted);
    $("#voiceOutputButton").setAttribute("aria-pressed", String(voice.speakerMuted));
    $("#voiceOutputIcon").textContent = voice.speakerMuted ? "🔇" : "🔊";
    $("#voiceOutputLabel").textContent = translate(voice.speakerMuted ? "Sound off" : "Sound on");
    $("#voiceStageShareButton").classList.toggle("hidden", !activeCall || !["connected", "reconnecting"].includes(voice.status));
    $("#voiceRefreshScreenButton").classList.toggle("hidden", !activeCall || !voice.screenSharing);
    $("#voiceRefreshScreenButton").disabled = !!voice.screenStarting;
    $("#voiceStageShareIcon").textContent = voice.screenSharing ? "■" : "▣";
    $("#voiceStageShareLabel").textContent = translate(voice.screenStarting ? "Starting…" : (voice.screenSharing ? "Stop sharing" : "Share screen"));
    $("#voiceStageShareButton").disabled = !!voice.screenStarting;
    $("#voiceStageAccept").classList.toggle("hidden", groupMode || voice.status !== "incoming");
    $("#voiceStageAccept").textContent = translate("Join call");
    $("#voiceStageDecline").classList.toggle("hidden", groupMode || voice.status !== "incoming");
    $("#voiceHangupButton").querySelector("span:last-child").textContent = translate(groupMode ? "Leave call" : "End call");
    $("#voiceHangupButton").classList.toggle("hidden", !activeCall);
    const shareQualityNotice = !groupMode && voice.screenSharing && voice.screenQuality
      ? `GhostShare · ${translate(voice.screenQuality.label)}${voice.screenQuality.state === "network-adjusted" ? ` · ${translate("connection adjusted")}` : voice.screenQuality.state === "recovered" ? ` · ${translate("quality recovered")}` : ""}.`
      : "";
    $("#voiceDeviceNotice").textContent = translate(shareQualityNotice || (groupMode && activeCall
      ? translate(!voice.micAvailable ? "Group voice is relayed by a LiveKit SFU and is not end-to-end encrypted. No microphone was found; you joined muted." : "Group voice is relayed by a LiveKit SFU and is not end-to-end encrypted. Your microphone is off.")
      : voice.status === "incoming"
      ? "Join the call when you are ready. If no microphone is available, you can still listen muted."
      : !voice.micAvailable && activeCall
        ? (voice.micUnavailableReason || "No microphone found. You joined muted and can still listen.")
        : voice.pushToTalk
          ? "Push to talk is on. Hold Space to speak; your microphone stays muted otherwise."
          : voice.micMuted && activeCall
            ? "Your microphone is off. You can still hear the call."
            : activeCall
              ? "Your microphone is on. Use the controls above to switch devices or mute."
              : "Choose your microphone and speaker, or join muted and listen."));
    $(".voice-encryption-note").textContent = groupMode ? translate("RELAYED · NOT END-TO-END ENCRYPTED") : "LIVE · AUDIO";
    const hasRemoteVideo = !!voice.remoteVideoStream?.getVideoTracks().some(track => track.readyState === "live" && !track.muted);
    const hasLocalVideo = !!voice.localScreenStream?.getVideoTracks().some(track => track.readyState === "live" && !track.muted);
    $("#remoteScreenPanel").classList.toggle("hidden", groupMode || !hasRemoteVideo);
    $("#localScreenPanel").classList.toggle("hidden", groupMode || !voice.screenSharing || !hasLocalVideo);
    $("#groupVoiceMedia").classList.toggle("hidden", !groupMode || !$("#groupVoiceMedia .group-screen-tile"));
    $("#screenShareLayout").classList.toggle("has-remote", hasRemoteVideo);
    $("#screenShareLayout").classList.toggle("is-group", groupMode);
    const showingCallConversation = state.page === "messages" && state.active?.id === voiceConversationId;
    $("#voiceStage").classList.toggle("hidden", voice.status === "idle" || !showingCallConversation);
    $("#voiceStage").classList.toggle("has-screen-share", !groupMode && (hasRemoteVideo || hasLocalVideo));
    const dock = $("#activeCallDock");
    dock.classList.toggle("hidden", voice.status === "idle" || showingCallConversation);
    dock.dataset.conversationId = voiceConversationId || "";
    $("#activeCallName").textContent = peer;
    setAvatar($("#activeCallAvatar"), peer, conversation?.avatar_data);
    $("#activeCallStatus").textContent = `${translate(labels[voice.status] || "Voice call")}${voice.screenSharing ? ` · ${translate("Sharing screen")}` : ""}`;
    $("#returnToCall").disabled = !voiceConversationId;
    $("#activeCallMic").classList.toggle("hidden", !activeCall);
    $("#activeCallMic").textContent = $("#voiceStageMicLabel").textContent;
    $("#activeCallMic").setAttribute("aria-pressed", String(voice.micMuted));
    $("#activeCallEnd").textContent = translate(voice.status === "incoming" ? "Decline" : groupMode ? "Leave call" : "End call");
    if (voice.status === "idle") {
      closeVoiceParticipantMenu();
      if (!groupMode) {
        $("#remoteAudio").srcObject = null;
        $("#remoteScreen").srcObject = null;
        $("#localScreen").srcObject = null;
        $("#enableCallAudio").classList.add("hidden");
      }
    }
  }

  function onVoiceChange(voice) {
    voiceUiState = voice;
    const processingStatus = $("#voiceProcessingStatus");
    if (processingStatus) {
      const processing = voice.audioProcessing;
      if (!processing) {
        processingStatus.textContent = translate("Processing is checked when a call starts.");
      } else {
        const enabled = [
          processing.echoCancellation && translate("echo cancellation"),
          processing.noiseSuppression && translate("noise suppression"),
          processing.autoGainControl && translate("auto gain"),
          processing.voiceFocus && translate("voice focus"),
        ].filter(Boolean);
        processingStatus.textContent = enabled.length
          ? `${translate("Active:")} ${enabled.join(" · ")}.`
          : translate("Your microphone is using the browser fallback. Check the selected device and Windows microphone effects.");
      }
    }
    renderVoiceControls();
    syncVoiceSpeakingDetection(voice);
    const audio = $("#remoteAudio");
    const video = $("#remoteScreen");
    const localVideo = $("#localScreen");
    if (audio.srcObject !== voice.remoteAudioStream) {
      audio.srcObject = voice.remoteAudioStream;
      if (voice.remoteAudioStream) void audio.play().then(() => $("#enableCallAudio").classList.add("hidden")).catch(() => $("#enableCallAudio").classList.remove("hidden"));
    }
    const remoteScreenStream = voice.remoteVideoStream?.getVideoTracks().some(track => track.readyState === "live" && !track.muted) ? voice.remoteVideoStream : null;
    const localScreenStream = voice.localScreenStream?.getVideoTracks().some(track => track.readyState === "live" && !track.muted) ? voice.localScreenStream : null;
    if (video.srcObject !== remoteScreenStream) {
      video.srcObject = remoteScreenStream;
      if (remoteScreenStream) void video.play().catch(() => {});
    }
    if (localVideo.srcObject !== localScreenStream) {
      localVideo.srcObject = localScreenStream;
      if (localScreenStream) void localVideo.play().catch(() => {});
    }
    void applyPlaybackSettings();
  }

  function setupVoiceClient() {
    voiceClient = window.GhostChatVoice.create({
      getSettings: voiceSettings,
      getUserId: () => state.user?.id || "",
      getIceConfiguration: async () => {
        try { return await request("/v1/voice/ice"); }
        catch (error) {
          if (error.status === 404) {
            throw new Error("This server is outdated for voice and screen sharing. Restart it with the latest GhostChat server build.");
          }
          throw error;
        }
      },
      getShareToken: conversationId => request(`/v1/conversations/${encodeURIComponent(conversationId)}/share-token`, { method: "POST" }),
      sendSignal: ({ conversationId, peerId, kind, payload }) => {
        if (!state.socket || state.socket.readyState !== WebSocket.OPEN) throw new Error("The chat server is reconnecting. Wait until it is connected and try again.");
        state.socket.send(JSON.stringify({ type: "voice_signal", conversation_id: conversationId, to_user_id: peerId, kind, payload }));
      },
      onChange: onVoiceChange,
      onIncoming: ({ conversationId }) => {
        const convo = state.conversations.find(item => item.id === conversationId);
        if (convo) {
          void openConversation(conversationId);
          $("#pageContent").classList.add("hidden");
          renderSidebar();
        }
        const caller = convo?.display_name || "Your friend";
        toast(`${caller} is calling.`);
        void sendDesktopNotification("Incoming GhostChat call", `${caller} is calling you.`);
      },
      onError: message => toast(message, true),
    });
    groupVoiceClient = window.GhostChatGroupVoice.create({
      getSettings: voiceSettings,
      getUserId: () => state.user?.id || "",
      getDisplayName: () => state.user?.display_name || translate("You"),
      getAvatar: () => state.user?.avatar_data || null,
      isPeerMuted: accountId => isGroupPeerMuted(groupVoiceUiState.conversationId, accountId),
      getMembers: conversationId => request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`),
      getToken: conversationId => request(`/v1/conversations/${encodeURIComponent(conversationId)}/voice-token`, { method: "POST" }),
      onChange: voice => { groupVoiceUiState = voice; renderVoiceControls(); },
      onAudioBlocked: () => toast("Call audio is still blocked by Windows or the audio device.", true),
      translate,
    });
    renderVoiceControls();
  }

  async function authenticateReply(reply, recoveredKey = null) {
    await storeToken(reply.session_token); state.user = reply.user; state.cryptoDevice = null;
    if (reply.recovery_token) {
      try { await saveRecoveryKey(reply.recovery_token); }
      catch (error) { console.warn("Could not save the recovery key in Windows Credential Manager", error); toast(translate("Windows could not store a secure copy. Copy or save the key before continuing."), true); }
      await savePendingRecovery(reply.recovery_token); $("#recoveryValue").textContent = reply.recovery_token; $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView");
    } else {
      if (recoveredKey) {
        try { await saveRecoveryKey(recoveredKey); }
        catch (error) { console.warn("Could not save the recovered key in Windows Credential Manager", error); }
      }
      await clearPendingRecovery(); await enterApp();
    }
  }
  function setAuthTab(tab, moveFocus = false) {
    const buttons = $$(".auth-tab");
    const selected = buttons.find(button => button.dataset.authTab === tab) || buttons[0];
    if (!selected) return;
    const activeTab = selected.dataset.authTab;
    for (const button of buttons) {
      const active = button === selected;
      button.classList.toggle("active", active);
      button.setAttribute("aria-selected", String(active));
      button.tabIndex = active ? 0 : -1;
    }
    for (const [name, panel] of [["create", $("#createForm")], ["recover", $("#recoverForm")]]) {
      const active = name === activeTab;
      panel.hidden = !active;
      panel.classList.toggle("hidden", !active);
      panel.setAttribute("aria-hidden", String(!active));
    }
    if (moveFocus) selected.focus({ preventScroll: true });
  }

  $$(".auth-tab").forEach(button => button.addEventListener("click", () => setAuthTab(button.dataset.authTab)));
  $(".auth-tabs").addEventListener("keydown", event => {
    const tabs = $$(".auth-tab");
    const index = tabs.indexOf(document.activeElement);
    if (index < 0 || !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
    setAuthTab(tabs[nextIndex].dataset.authTab, true);
  });
  $("#signupAvatar").addEventListener("change", event => { void handleAvatarFile(event.currentTarget, "#signupAvatarPreview", setSignupAvatar); });
  $("#profileAvatarInput").addEventListener("change", event => { void handleAvatarFile(event.currentTarget, "#profileAvatarPreview", value => { profileAvatarData = value; }); });
  $("#createForm").addEventListener("submit", async event => {
    event.preventDefault(); const form = new FormData(event.currentTarget); const button = event.currentTarget.querySelector("button[type=submit]"); button.disabled = true;
    try { const reply = await request("/v1/accounts", { method: "POST", body: JSON.stringify({ display_name: form.get("display_name"), avatar_data: signupAvatarData }) }); await authenticateReply(reply); }
    catch (error) { toast(error.message, true); } finally { button.disabled = false; }
  });
  $("#recoverForm").addEventListener("submit", async event => {
    event.preventDefault(); const form = new FormData(event.currentTarget); const button = event.currentTarget.querySelector("button[type=submit]"); button.disabled = true;
    try { const recoveryKey = form.get("recovery_token").trim(); const reply = await request("/v1/sessions/recover", { method: "POST", body: JSON.stringify({ recovery_token: recoveryKey }) }); await authenticateReply(reply, recoveryKey); }
    catch (error) { toast(error.message, true); } finally { button.disabled = false; }
  });
  $("#copyRecovery").onclick = async () => { try { await navigator.clipboard.writeText($("#recoveryValue").textContent); toast("Recovery key copied."); } catch { toast("Copy was blocked. Select and copy the key manually.", true); } };
  $("#downloadRecovery").onclick = () => { const text = `GhostChat recovery key\n\n${$("#recoveryValue").textContent}\n\nKeep this key private. Anyone with it can recover the account. GhostChat cannot reset it.`; const url = URL.createObjectURL(new Blob([text], { type: "text/plain" })); const link = create("a"); link.href = url; link.download = "ghostchat-recovery.txt"; link.click(); URL.revokeObjectURL(url); };
  $("#recoveryConfirm").onchange = event => { $("#continueAfterRecovery").disabled = !event.target.checked; };
  $("#continueAfterRecovery").onclick = async () => {
    if (state.pendingRecovery) {
      try { await saveRecoveryKey(state.pendingRecovery); }
      catch (error) { console.warn("Could not save the recovery key in Windows Credential Manager", error); }
    }
    await clearPendingRecovery(); if (state.token) await enterApp(); else show("authView");
  };
  $$(".rail-button[data-page]").forEach(button => button.addEventListener("click", () => setPage(button.dataset.page)));
  $("#sidebarToggleButton").onclick = event => {
    const open = $("#appView").classList.toggle("sidebar-open");
    event.currentTarget.setAttribute("aria-expanded", String(open));
    event.currentTarget.setAttribute("aria-label", translate(open ? "Hide conversations" : "Show conversations"));
    event.currentTarget.title = translate(open ? "Hide conversations" : "Show conversations");
  };
  $("#sidebarContent").addEventListener("click", event => {
    if (!event.target.closest(".list-row")) return;
    closeMobileSidebar();
  });
  $("#sidebarAction").onclick = () => {
    if (state.page === "messages") $("#newConversationDialog").showModal();
    else setPage("search");
  };
  $("#findPeopleAction").onclick = () => { $("#newConversationDialog").close(); setPage("search"); };
  $("#createGroupAction").onclick = () => { $("#newConversationDialog").close(); openGroupDialog(); };
  $("#manageGroupButton").onclick = () => void openGroupManageDialog();
  $("#saveGroupName").onclick = async event => {
    if (!state.active || state.active.kind !== "group") return;
    const button = event.currentTarget; button.disabled = true;
    try {
      const conversationId = state.active.id;
      await request(`/v1/conversations/${encodeURIComponent(conversationId)}/group`, { method: "PATCH", body: JSON.stringify({ name: $("#manageGroupName").value.trim() }) });
      await refreshConversations(); await openConversation(conversationId, false); await refreshGroupManageDialog(); toast(translate("Group name saved."));
    } catch (error) { toast(error.message || translate("Could not update the group."), true); }
    finally { button.disabled = false; }
  };
  $("#inviteGroupFriends").onclick = async event => {
    if (!state.active || state.active.kind !== "group") return;
    const memberIds = $$("#groupInviteList input:checked").map(input => input.value);
    if (!memberIds.length) return;
    const button = event.currentTarget; button.disabled = true;
    $("#groupManageStatus").textContent = translate("Adding friends…");
    try {
      const conversationId = state.active.id;
      const conversation = state.active;
      const cryptoLock = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/crypto-lock`);
      if (cryptoLock.locked) {
        if (!cryptoLock.setupComplete || !state.cryptoEnabled.has(conversationId)) {
          throw new Error(translate("This encrypted group can only be changed from a device that has joined its MLS group."));
        }
        const pending = await invoke("crypto_get_pending_group_membership_update", { userId: state.user.id, conversationId });
        if (pending) {
          const pendingMembers = [...(pending.memberIds || [])].sort();
          const selectedMembers = [...memberIds].sort();
          if (pending.action !== "add" || pendingMembers.length !== selectedMembers.length || pendingMembers.some((id, index) => id !== selectedMembers[index])) {
            throw new Error(translate("Finish the pending encrypted membership update before starting another one."));
          }
          await publishGroupMembershipUpdate(conversation, pending);
        } else {
          const members = await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`);
          const expectedMemberIds = members.map(member => member.id).sort();
          const keyPackages = [];
          const deviceCodes = [];
          for (const memberId of memberIds) {
            const friend = state.friends.find(item => item.id === memberId);
            if (!friend) throw new Error(translate("Choose accepted friends from the list."));
            const claimed = await request(`/v1/users/${encodeURIComponent(friend.id)}/crypto/key-package`, {
              method: "POST", body: JSON.stringify({ client_claim_id: crypto.randomUUID() }),
            });
            const inspected = await invoke("crypto_inspect_key_package", {
              peerUserId: friend.id, peerDeviceId: claimed.device_id, payload: claimed.payload,
            });
            keyPackages.push({ userId: friend.id, deviceId: claimed.device_id, payload: claimed.payload });
            deviceCodes.push(`${friend.display_name} (@${friend.username})\n${inspected.identityFingerprint}`);
          }
          const approved = confirm(`${translate("Verify each selected friend's full security code through another trusted channel before adding them to this encrypted group.")}\n\n${deviceCodes.join("\n\n")}`);
          if (!approved) {
            $("#groupManageStatus").textContent = "";
            toast(translate("Encryption update cancelled. The one-time key packages were already claimed; the selected friends may need to publish fresh packages."));
            return;
          }
          const update = await invoke("crypto_prepare_group_membership_update", {
            userId: state.user.id, conversationId, expectedMemberIds, peers: keyPackages,
          });
          await publishGroupMembershipUpdate(conversation, update);
        }
      } else {
        await request(`/v1/conversations/${encodeURIComponent(conversationId)}/members`, {
          method: "POST", body: JSON.stringify({ member_ids: memberIds }),
        });
      }
      await refreshConversations(); await openConversation(conversationId, false); await refreshGroupManageDialog(); toast(translate("Friends added to the group."));
    } catch (error) { $("#groupManageStatus").textContent = error.message || translate("Could not update the group."); }
    finally { button.disabled = false; }
  };
  $("#generateGroupInvite").onclick = async event => {
    if (!state.active || state.active.kind !== "group") return;
    const button = event.currentTarget; button.disabled = true;
    try {
      const invite = await request(`/v1/conversations/${encodeURIComponent(state.active.id)}/invites`, { method: "POST", body: JSON.stringify({ max_uses: 20 }) });
      state.groupInviteCodes.set(invite.id, invite.code);
      toast(translate("Group invite code created.")); await refreshGroupManageDialog();
    } catch (error) { toast(error.message, true); }
    finally { button.disabled = false; }
  };
  $("#joinGroupAction").onclick = () => { $("#newConversationDialog").close(); $("#groupInviteCodeInput").value = ""; $("#joinGroupDialog").showModal(); setTimeout(() => $("#groupInviteCodeInput").focus(), 20); };
  $("#joinGroupForm").addEventListener("submit", async event => {
    event.preventDefault();
    const button = $("#submitGroupInvite"); button.disabled = true; button.textContent = translate("Joining…");
    try {
      const code = $("#groupInviteCodeInput").value.trim();
      const conversation = await request("/v1/group-invites/join", { method: "POST", body: JSON.stringify({ code }) });
      $("#joinGroupDialog").close(); await refreshConversations(); if (state.page !== "messages") setPage("messages"); await openConversation(conversation.id); toast(translate("Joined group."));
    } catch (error) { toast(error.message, true); }
    finally { button.disabled = false; button.textContent = translate("Join group"); }
  });
  window.__TAURI__?.event?.listen?.("ghostchat:group-invite-ready", () => { void processPendingGroupInvites(); });
  $("#groupNameInput").addEventListener("input", updateGroupSubmitState);
  $("#groupForm").addEventListener("submit", async event => {
    event.preventDefault();
    const button = $("#submitGroup"); button.disabled = true;
    const member_ids = $$("#groupFriendList input:checked").map(input => input.value);
    try {
      const convo = await request("/v1/conversations/groups", { method: "POST", body: JSON.stringify({ name: $("#groupNameInput").value.trim(), member_ids }) });
      $("#groupDialog").close(); await refreshConversations(); if (state.page !== "messages") setPage("messages"); await openConversation(convo.id); toast(translate("Group created."));
    } catch (error) { toast(error.message, true); }
    finally { updateGroupSubmitState(); }
  });
  $("#searchInChat").onclick = () => { state.searchTab = "users"; setPage("searchMessages"); renderPage(); };
  $("#userSafetyButton").onclick = openSafetyDialog;
  $("#safetyBlockButton").onclick = async () => {
    const peerId = state.active?.peer_id;
    if (!peerId) return;
    const wasBlocked = state.blockedUserIds.has(peerId);
    try {
      await request(`/v1/users/${encodeURIComponent(peerId)}/block`, { method: wasBlocked ? "DELETE" : "POST" });
      if (wasBlocked) state.blockedUserIds.delete(peerId); else state.blockedUserIds.add(peerId);
      if (!wasBlocked && state.active?.kind === "direct" && state.active.peer_id === peerId) voiceClient?.hangup();
      const nowBlocked = state.blockedUserIds.has(peerId);
      renderVoiceControls();
      const blockButton = $("#safetyBlockButton");
      blockButton.textContent = translate(nowBlocked ? "Unblock user" : "Block user");
      blockButton.classList.toggle("danger", !nowBlocked);
      blockButton.classList.toggle("secondary", nowBlocked);
      await refreshFriends({ notifyNew: false }); await refreshConversations();
      toast(translate(wasBlocked ? "You unblocked this user. Send a new friend request to start a new direct chat." : "You blocked this user. Existing chat history is still available."));
    } catch (error) { toast(error.message, true); }
  };
  $("#safetyReportForm").addEventListener("submit", async event => {
    event.preventDefault();
    const peerId = state.active?.peer_id;
    if (!peerId) return;
    const submit = $("#safetyReportForm button[type=submit]");
    submit.disabled = true;
    try {
      await request(`/v1/users/${encodeURIComponent(peerId)}/report`, {
        method: "POST",
        body: JSON.stringify({ category: $("#safetyReportCategory").value, details: $("#safetyReportDetails").value, conversation_id: state.active.id })
      });
      $("#userSafetyDialog").close();
      toast(translate("Report sent. No chat messages were attached."));
    } catch (error) { toast(error.message, true); }
    finally { submit.disabled = false; }
  });
  $("#composer").addEventListener("submit", async event => { event.preventDefault(); const input = $("#messageInput"); const content = input.value; if (!content.trim()) return; const reply = state.replyingTo; input.value = ""; input.style.height = "auto"; if (await sendMessage(content, crypto.randomUUID(), reply?.id || null) && state.replyingTo?.id === reply?.id) setReplyTarget(null); });
  $("#cancelReply").onclick = () => setReplyTarget(null);
  $("#attachFileButton").onclick = () => {
    if (state.active && state.cryptoEnabled.has(state.active.id)) void uploadEncryptedAttachment();
    else $("#attachmentInput").click();
  };
  $("#attachmentInput").addEventListener("change", event => { const file = event.target.files?.[0]; if (file) void uploadAttachment(file); });
  $("#messageInput").addEventListener("keydown", event => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); $("#composer").requestSubmit(); } });
  $("#messageInput").addEventListener("input", event => { event.target.style.height = "auto"; event.target.style.height = `${Math.min(event.target.scrollHeight, 180)}px`; sendTyping(!!event.target.value.trim()); });
  window.addEventListener("focus", () => { if (state.active) void loadMessages(false); });
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible" && state.active) void loadMessages(false); });
  $("#loadOlder").onclick = () => loadMessages(true);
  function resetSettingsRecoveryKey(secret) {
    state.recoveryKey = secret || "";
    const available = !!state.recoveryKey;
    settingsRecoveryStatusKey = available ? "Recovery key saved securely on this device." : "This device has no saved key. Creating a replacement will disable your old key.";
    $("#settingsRecoveryStatus").textContent = translate(settingsRecoveryStatusKey);
    $("#settingsRecoveryValue").value = state.recoveryKey;
    $("#settingsRecoveryValue").classList.add("hidden");
    $("#settingsRecoveryTools").classList.add("hidden");
    $("#showSettingsRecoveryKey").classList.toggle("hidden", !available);
    $("#showSettingsRecoveryKey").textContent = translate("Show recovery key");
  }
  async function openSettings() {
    const inMemoryRecoveryKey = state.recoveryKey;
    $$(".settings-section").forEach(section => { if (section.querySelector("#profileNameInput")) section.classList.toggle("hidden", !state.user); });
    $("#recoverySettingsPanel").classList.toggle("hidden", !state.user);
    $("#encryptionSettingsSection").classList.toggle("hidden", !state.user);
    $("#serverAddressInput").value = state.base;
    $("#profileNameInput").value = state.user?.display_name || "";
    $("#profileUsernameInput").value = state.user?.username || "";
    $("#presenceMode").value = state.presenceMode || "online";
    profileAvatarData = state.user?.avatar_data || null;
    setAvatar($("#profileAvatarPreview"), state.user?.display_name, profileAvatarData);
    $("#languageSelect").value = tr() ? "tr" : "en";
    $("#desktopNotificationsEnabled").checked = localStorage.getItem("ghostchat.desktopNotifications") !== "false";
    try { $("#startWithWindows").checked = !!(await invoke("get_start_at_login")); }
    catch (error) { console.warn("Could not read the Windows startup setting", error); $("#startWithWindows").checked = false; }
    populateVoiceSettings();
    resetSettingsRecoveryKey("");
    $("#settingsDialog").showModal();
    if (state.user) resetSettingsRecoveryKey(await loadRecoveryKey(inMemoryRecoveryKey));
    if (state.user && !state.cryptoDevice) {
      try {
        const savedDevice = JSON.parse(localStorage.getItem(cryptoDeviceStorageKey()) || "null");
        if (savedDevice?.deviceId && savedDevice?.identityFingerprint) state.cryptoDevice = savedDevice;
      } catch {}
    }
    if (state.cryptoDevice) {
      showCryptoDevice(state.cryptoDevice);
      $("#cryptoDeviceStatus").textContent = translate("A local device identity is saved. Publish a fresh one-time key package when needed.");
    }
  }
  $("#settingsButton").onclick = () => { closeMobileSidebar(); openSettings(); };
  $("#serverSettings").onclick = () => {
    openSettings();
    $("#serverAddressInput").focus({ preventScroll: true });
    $("#serverAddressInput").scrollIntoView({ block: "center", behavior: "smooth" });
  };
  $("#profileButton").onclick = openSettings;
  for (const selector of ["#languageSelect", "#authLanguageSelect"]) $(selector).onchange = event => { localStorage.setItem("ghostchat.language", event.target.value); applyLanguage(); renderSidebar(); if (["friends", "search", "searchMessages"].includes(state.page)) renderPage(); else if (!state.active) showWelcome(); renderVoiceControls(); };
  for (const selector of ["#themeSelect", "#textSize", "#motionSelect"]) $(selector).onchange = () => applyAppearance({ persist: true });
  $("#presenceMode").onchange = event => { void setPresence(event.target.value); };
  $("#themeWallpaperInput").onchange = async event => {
    const file = event.target.files?.[0]; if (!file) return;
    try {
      const wallpaper = await prepareWallpaper(file);
      localStorage.setItem("ghostchat.wallpaper", wallpaper);
      applyAppearance(); updateWallpaperStatus();
      toast(translate("Custom wallpaper applied on this device."));
    } catch (error) { toast(error.message || translate("Could not use that wallpaper."), true); }
    finally { event.target.value = ""; }
  };
  $("#themeWallpaperShade").oninput = event => {
    localStorage.setItem("ghostchat.wallpaperShade", event.target.value);
    applyAppearance();
  };
  $("#clearThemeWallpaper").onclick = () => {
    localStorage.removeItem("ghostchat.wallpaper");
    applyAppearance(); updateWallpaperStatus();
    toast(translate("Custom wallpaper removed."));
  };
  $("#removeProfileAvatar").onclick = () => { profileAvatarData = null; setAvatar($("#profileAvatarPreview"), state.user?.display_name); };
  $("#showSettingsRecoveryKey").onclick = event => {
    const value = $("#settingsRecoveryValue");
    const reveal = value.classList.contains("hidden");
    value.classList.toggle("hidden", !reveal);
    $("#settingsRecoveryTools").classList.toggle("hidden", !reveal);
    event.currentTarget.textContent = translate(reveal ? "Hide recovery key" : "Show recovery key");
    if (reveal) value.select();
  };
  $("#copySettingsRecoveryKey").onclick = async () => {
    try { await navigator.clipboard.writeText(state.recoveryKey); toast(translate("Recovery key copied.")); }
    catch { toast("Copy was blocked. Select and copy the key manually.", true); $("#settingsRecoveryValue").focus(); $("#settingsRecoveryValue").select(); }
  };
  $("#downloadSettingsRecoveryKey").onclick = () => {
    const text = `GhostChat recovery key\n\n${state.recoveryKey}\n\nKeep this key private. Anyone with it can recover the account.`;
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const link = create("a"); link.href = url; link.download = "ghostchat-recovery.txt"; link.click(); URL.revokeObjectURL(url);
  };
  $("#generateRecoveryKey").onclick = async event => {
    if (!confirm(translate("Create a replacement recovery key? Your old key will stop working immediately."))) return;
    const button = event.currentTarget; button.disabled = true;
    try {
      const reply = await request("/v1/me/recovery-key", { method: "POST" });
      resetSettingsRecoveryKey(reply.recovery_token);
      $("#settingsRecoveryValue").classList.remove("hidden");
      $("#settingsRecoveryTools").classList.remove("hidden");
      $("#showSettingsRecoveryKey").classList.remove("hidden");
      $("#showSettingsRecoveryKey").textContent = translate("Hide recovery key");
      try {
        await saveRecoveryKey(reply.recovery_token);
        settingsRecoveryStatusKey = "A new recovery key is ready. Save a backup somewhere safe.";
        $("#settingsRecoveryStatus").textContent = translate(settingsRecoveryStatusKey);
      } catch (error) {
        console.warn("Could not save the recovery key in Windows Credential Manager", error);
        settingsRecoveryStatusKey = "Windows could not save this key. Save the displayed backup before closing Settings.";
        $("#settingsRecoveryStatus").textContent = translate(settingsRecoveryStatusKey);
        toast(translate("Windows could not save this key. Save the displayed backup before closing Settings."), true);
      }
    } catch (error) { toast(translate(error.message || "Could not create a recovery key."), true); }
    finally { button.disabled = false; }
  };
  $("#saveProfile").onclick = async event => {
    const button = event.currentTarget; button.disabled = true;
    try {
      state.user = await request("/v1/me/profile", { method: "PATCH", body: JSON.stringify({ display_name: $("#profileNameInput").value, username: $("#profileUsernameInput").value, avatar_data: profileAvatarData }) });
      updateIdentity(); renderVoiceControls(); await refreshFriends({ notifyNew: false }); await refreshConversations();
      if (state.active) { await loadMessages(false); }
      toast(translate("Profile saved."));
    } catch (error) { toast(translate(error.message), true); }
    finally { button.disabled = false; }
  };
  function showCryptoDevice(device) {
    $("#cryptoDeviceDetails").classList.remove("hidden");
    $("#cryptoDeviceIdValue").textContent = device.deviceId;
    $("#cryptoFingerprintValue").textContent = device.identityFingerprint;
    localStorage.setItem(cryptoDeviceStorageKey(), JSON.stringify({ deviceId: device.deviceId, identityFingerprint: device.identityFingerprint }));
  }
  function formatDeviceDate(value) {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString(tr() ? "tr-TR" : undefined);
  }
  async function loadCryptoDevices() {
    const button = $("#refreshCryptoDevices");
    const list = $("#cryptoDeviceList");
    if (!state.user || !button || !list) return;
    button.disabled = true;
    list.replaceChildren(create("div", "empty-list", "Loading encryption devices…"));
    try {
      const devices = await request("/v1/me/crypto/devices");
      list.replaceChildren();
      if (!devices.length) {
        list.append(create("div", "empty-list", "No encryption devices registered."));
        return;
      }
      const currentId = state.cryptoDevice?.deviceId || "";
      for (const device of devices) {
        const isCurrent = device.id === currentId;
        const revoked = !!device.revoked_at;
        const row = create("div", "crypto-device-row");
        const copy = create("div", "list-copy");
        copy.append(create("strong", "", device.label || translate("Encryption device")));
        const status = revoked ? translate("Revoked") : translate("Active");
        copy.append(create("span", "", `${status}${isCurrent ? ` · ${translate("Current device")}` : ""}`));
        copy.append(create("span", "", `${translate("Registered")}: ${formatDeviceDate(device.created_at)}`));
        row.append(copy);
        if (!revoked && !isCurrent) {
          const revoke = create("button", "button danger crypto-device-revoke", "Revoke device");
          revoke.type = "button";
          revoke.onclick = async () => {
            if (!confirm(translate("Revoke this encryption device ID? It will not be able to claim unclaimed key packages on this server. Its app sessions and account access remain active, packages already claimed stay valid, and existing encrypted-group membership is unchanged."))) return;
            revoke.disabled = true;
            try {
              await request(`/v1/me/crypto/devices/${encodeURIComponent(device.id)}`, { method: "DELETE" });
              toast(translate("Encryption device revoked."));
              await loadCryptoDevices();
            } catch (error) {
              toast(error.message || translate("Encryption devices could not be loaded."), true);
              revoke.disabled = false;
            }
          };
          row.append(revoke);
        }
        list.append(row);
      }
    } catch (error) {
      list.replaceChildren(create("div", "empty-list", error.message || translate("Encryption devices could not be loaded.")));
    } finally {
      button.disabled = false;
    }
  }
  function cryptoDeviceStorageKey() { return `ghostchat.cryptoDevice.${state.base || "local"}.${state.user?.id || ""}`; }
  $("#refreshCryptoDevices").onclick = () => void loadCryptoDevices();
  $("#prepareCryptoDeviceButton").onclick = async event => {
    const button = event.currentTarget;
    if (!state.user) return;
    button.disabled = true;
    const status = $("#cryptoDeviceStatus");
    status.textContent = translate("Preparing this device…");
    try {
      const device = await invoke("crypto_prepare_device", { userId: state.user.id });
      await request("/v1/me/crypto/devices", { method: "POST", body: JSON.stringify({ id: device.deviceId, label: device.label }) });
      try {
        await request(`/v1/me/crypto/devices/${encodeURIComponent(device.deviceId)}/key-packages`, {
          method: "POST",
          body: JSON.stringify({ package_ref: device.packageRef, payload: device.payload }),
        });
      } catch (error) {
        state.cryptoDevice = device;
        showCryptoDevice(device);
        status.textContent = translate("The device identity was prepared, but its key package could not be published.");
        throw error;
      }
      state.cryptoDevice = device;
      showCryptoDevice(device);
      status.textContent = translate("Encryption device is ready; one key package was published.");
    } catch (error) {
      if (!state.cryptoDevice) status.textContent = error.message || translate("The device could not be prepared.");
      toast(error.message || "The device could not be prepared.", true);
    } finally { button.disabled = false; }
  };
  $("#desktopNotificationsEnabled").onchange = event => localStorage.setItem("ghostchat.desktopNotifications", String(event.target.checked));
  $("#testDesktopNotification").onclick = async () => {
    const button = $("#testDesktopNotification"); button.disabled = true;
    try {
      await invoke("show_desktop_notification", { title: "GhostChat notifications", body: "Windows notifications are working." });
      toast("Test notification sent. If it is hidden, check Windows notification settings.");
    } catch (error) { toast(error.message || "Windows could not show the test notification.", true); }
    finally { button.disabled = false; }
  };
  $("#refreshAudioDevices").onclick = refreshAudioDevices;
  $("#voiceRefreshDevices").onclick = () => void refreshAudioDevices();
  $("#voiceMicTestButton").onclick = () => void toggleMicrophoneTest();
  for (const selector of ["#voiceEchoCancellation", "#voiceNoiseSuppression", "#voiceAutoGain", "#voiceFocus", "#voiceFocusSensitivity"]) {
    $(selector).onchange = async () => {
      persistVoiceSettings();
      const updates = await Promise.allSettled([voiceClient?.applyAudioSettings?.(), groupVoiceClient?.applyAudioSettings?.()]);
      const failed = updates.find(result => result.status === "rejected");
      if (failed) toast(failed.reason?.message || "Could not apply the audio processing change.", true);
      else toast(translate("Audio processing applied."));
    };
  }
  $("#voicePeerCard").addEventListener("contextmenu", openVoiceParticipantMenu);
  $("#voicePeerCard").addEventListener("keydown", event => {
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) openVoiceParticipantMenu(event);
  });
  $("#voiceParticipantMuteAction").onclick = () => {
    if (!voiceUiState.peerId) return;
    localStorage.setItem(peerMuteKey(), String(!isPeerAudioMuted()));
    closeVoiceParticipantMenu({ restoreFocus: true });
    renderPeerMuteState();
    void applyPlaybackSettings();
    syncVoiceSpeakingDetection(voiceUiState);
  };
  $("#voiceParticipantVolume").oninput = event => {
    const value = Math.max(0, Math.min(100, Number(event.target.value) || 0));
    localStorage.setItem(peerVolumeKey(), String(value));
    $("#voiceParticipantVolumeValue").textContent = `${value}%`;
    void applyPlaybackSettings();
  };
  $("#voiceParticipantProfileAction").onclick = () => {
    const peer = state.conversations.find(item => item.peer_id === voiceUiState.peerId) || state.active;
    closeVoiceParticipantMenu();
    if (peer) showProfilePreview({ id: voiceUiState.peerId, display_name: peer.display_name, username: peer.username, avatar_data: peer.avatar_data });
  };
  document.addEventListener("pointerdown", event => {
    if (!$("#voiceParticipantMenu").contains(event.target) && !$("#voicePeerCard").contains(event.target)) closeVoiceParticipantMenu();
  });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && !$("#voiceParticipantMenu").classList.contains("hidden")) closeVoiceParticipantMenu({ restoreFocus: true });
  });
  $("#settingsDialog").addEventListener("close", () => {
    void stopMicrophoneTest();
    $("#settingsRecoveryValue").value = "";
    $("#settingsRecoveryValue").classList.add("hidden");
    $("#settingsRecoveryTools").classList.add("hidden");
  });
  function bindVoiceDeviceSelect(selectId, companionId, storageKey, kind) {
    $(selectId).addEventListener("change", async event => {
      const deviceId = event.currentTarget.value;
      if (kind === "microphone") await stopMicrophoneTest();
      localStorage.setItem(storageKey, deviceId);
      const companion = $(companionId);
      if (companion && [...companion.options].some(option => option.value === deviceId)) companion.value = deviceId;
      try {
        if (kind === "microphone" && ["calling", "connecting", "connected", "reconnecting"].includes(voiceClient.getState().status)) {
          await voiceClient.setMicrophoneDevice(deviceId);
        }
        if (kind === "microphone" && ["connecting", "connected", "reconnecting"].includes(groupVoiceUiState.status)) {
          await groupVoiceClient.setMicrophoneDevice(deviceId);
        } else if (kind === "speaker") await applyPlaybackSettings();
      } catch (error) { toast(error.message || "Could not change the audio device.", true); }
      renderVoiceControls();
    });
  }
  bindVoiceDeviceSelect("#voiceMicSelect", "#voiceCallMicSelect", "ghostchat.voice.microphone", "microphone");
  bindVoiceDeviceSelect("#voiceCallMicSelect", "#voiceMicSelect", "ghostchat.voice.microphone", "microphone");
  bindVoiceDeviceSelect("#voiceSpeakerSelect", "#voiceCallSpeakerSelect", "ghostchat.voice.speaker", "speaker");
  bindVoiceDeviceSelect("#voiceCallSpeakerSelect", "#voiceSpeakerSelect", "ghostchat.voice.speaker", "speaker");
  $("#voiceCallButton").onclick = () => {
    if (state.active) {
      $("#voiceCallButton").disabled = true;
      const start = state.active.kind === "group" ? groupVoiceClient.join(state.active) : voiceClient.start(state.active);
      void start.catch(error => toast(error.message, true)).finally(renderVoiceControls);
    }
  };
  $("#voiceStageAccept").onclick = () => {
    $("#voiceStageAccept").disabled = true;
    void voiceClient.accept().catch(error => toast(error.message, true)).finally(() => { $("#voiceStageAccept").disabled = false; renderVoiceControls(); });
  };
  $("#voiceStageDecline").onclick = () => voiceClient.decline();
  function endCurrentCall() {
    if (groupVoiceUiState.status !== "idle") void groupVoiceClient.leave().catch(error => toast(error.message, true));
    else if (voiceClient.getState().status === "incoming") voiceClient.decline();
    else voiceClient.hangup();
  }
  function toggleCallMicrophone() {
    const action = groupVoiceUiState.status !== "idle" ? groupVoiceClient.toggleMicrophone() : voiceClient.toggleMicrophone();
    void action.catch(error => toast(error.message, true));
  }
  $("#voiceHangupButton").onclick = endCurrentCall;
  $("#activeCallEnd").onclick = endCurrentCall;
  $("#voiceStageMicButton").onclick = toggleCallMicrophone;
  $("#activeCallMic").onclick = toggleCallMicrophone;
  $("#returnToCall").onclick = async () => {
    const conversationId = $("#activeCallDock").dataset.conversationId;
    if (!conversationId) return;
    await openConversation(conversationId);
    const focusTarget = voiceClient.getState().status === "incoming" ? $("#voiceStageAccept") : $("#voiceStageMicButton");
    if (!focusTarget.closest(".hidden")) focusTarget.focus({ preventScroll: true });
  };
  $("#voiceOutputButton").onclick = () => {
    localStorage.setItem("ghostchat.voice.speakerMuted", String(!voiceSettings().speakerMuted));
    void applyPlaybackSettings();
    renderVoiceControls();
  };
  $("#voiceStageShareButton").onclick = () => {
    const action = groupVoiceUiState.status !== "idle" ? groupVoiceClient.toggleScreenSharing() : voiceClient.toggleScreenSharing();
    void action.catch(error => toast(error.message, true));
  };
  $("#voiceRefreshScreenButton").onclick = () => {
    const action = groupVoiceUiState.status !== "idle" ? groupVoiceClient.refreshScreenSharing() : voiceClient.refreshScreenSharing();
    void action.catch(error => toast(error.message, true));
  };
  $("#enableCallAudio").onclick = () => void $("#remoteAudio").play().then(() => $("#enableCallAudio").classList.add("hidden")).catch(() => toast("Call audio is still blocked by Windows or the audio device.", true));
  $("#voiceVolume").addEventListener("input", () => {
    const volume = $("#voiceVolume").value;
    $("#remoteAudio").volume = Number(volume) / 100;
    localStorage.setItem("ghostchat.voice.volume", volume);
    groupVoiceClient?.applyPlaybackSettings();
  });
  $("#changeServerButton").onclick = async event => {
    const button = event.currentTarget;
    let target = "";
    const input = $("#serverAddressInput").value.trim();
    if (input) {
      try { target = parseServerAddress(input); }
      catch (error) { toast(error.message || "Enter a valid server address.", true); return; }
    }
    if (target === state.base) { toast(tr() ? "Bu sunucu zaten seçili." : "This server is already selected.", true); return; }
    button.disabled = true;
    try {
      state.base = target;
      localStorage.setItem("ghostchat.server", target || LOCAL_SERVER_SENTINEL);
      state.groupVoiceAvailable = null;
      voiceClient?.hangup();
      void groupVoiceClient?.leave();
      const oldSocket = state.socket; state.socket = null; oldSocket?.close();
      state.user = null; state.recoveryKey = ""; state.active = null; state.friends = []; state.conversations = []; state.pending = [];
      state.cryptoDevice = null;
      resetCryptoViewState();
      updateServerLabels(); $("#settingsDialog").close();
      await restoreToken();
      try { state.pendingRecovery = await invoke("credential_get_pending_recovery", { serverKey: await serverKey(activeBase()) }) || ""; } catch { state.pendingRecovery = ""; }
      if (state.pendingRecovery) {
        try { await saveRecoveryKey(state.pendingRecovery); } catch (error) { console.warn("Could not promote the pending recovery key", error); }
        $("#recoveryValue").textContent = state.pendingRecovery;
        $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView");
      } else if (state.token) await enterApp(); else show("authView");
      if (target) {
        const controller = new AbortController(); const timeout = setTimeout(() => controller.abort(), 4000);
        let reachable = false;
        try { const response = await fetch(`${target}/health`, { signal: controller.signal }); reachable = response.ok; }
        catch {} finally { clearTimeout(timeout); }
        toast(reachable ? `Server address saved: ${new URL(target).host}` : `Server address saved: ${new URL(target).host}. The server is not reachable yet.`, !reachable);
      } else toast(tr() ? "Yerel sunucu seçildi." : "Server changed to the local server.");
    } catch (error) { toast(error.message || "Could not change the server.", true); }
    finally { button.disabled = false; }
  };
  $("#saveSettings").onclick = async () => {
    applyAppearance({ persist: true });
    try { await invoke("set_start_at_login", { enabled: $("#startWithWindows").checked }); }
    catch (error) { console.warn("Could not update the Windows startup setting", error); toast(tr() ? "Windows başlangıç ayarı kaydedilemedi; diğer ayarlar kaydedildi." : "Windows startup could not be changed; other settings were saved.", true); }
    applyLanguage();
    persistVoiceSettings(); await applyPlaybackSettings();
    const audioUpdates = await Promise.allSettled([
      voiceClient?.applyAudioSettings?.(),
      groupVoiceClient?.applyAudioSettings?.(),
    ]);
    const audioFailure = audioUpdates.find(result => result.status === "rejected");
    if (audioFailure) toast(audioFailure.reason?.message || "Could not apply audio processing settings.", true);
    const directVoiceActive = ["calling", "connecting", "connected", "reconnecting"].includes(voiceClient?.getState?.().status);
    const groupVoiceActive = ["connecting", "connected", "reconnecting"].includes(groupVoiceUiState.status);
    $("#settingsDialog").close(); toast(!audioFailure && (directVoiceActive || groupVoiceActive) ? "Voice processing settings applied." : "Settings saved.");
  };
  $("#exportSettings").onclick = () => {
    const data = { exportedAt: new Date().toISOString(), server: state.base || "local", theme: localStorage.getItem("ghostchat.theme") || "dark", textSize: localStorage.getItem("ghostchat.textSize") || "15", motion: localStorage.getItem("ghostchat.motion") || "full", wallpaperShade: localStorage.getItem("ghostchat.wallpaperShade") || "58", hasCustomWallpaper: !!localStorage.getItem("ghostchat.wallpaper"), language: localStorage.getItem("ghostchat.language") || "en" };
    downloadBlob(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }), "ghostchat-preferences.json");
    toast(translate("Preferences exported."));
  };
  $("#exportAccount").onclick = async () => { try { const data = await request("/v1/data/export"); downloadBlob(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }), "ghostchat-account-export.json"); toast(translate("Account data exported.")); } catch (error) { toast(error.message, true); } };
  $("#revokeOtherSessions").onclick = async event => {
    if (!confirm(translate("Sign out every other GhostChat session? This device will stay signed in."))) return;
    const button = event.currentTarget;
    button.disabled = true;
    try {
      await request("/v1/me/sessions/others", { method: "DELETE" });
      toast(translate("Other sessions were signed out."));
    } catch (error) {
      toast(error.message || translate("Other sessions could not be signed out."), true);
    } finally {
      button.disabled = false;
    }
  };
  $("#deleteAccount").onclick = async () => { if (!confirm("This permanently deletes your account, sessions, friend links, conversations, and stored messages. This cannot be undone.")) return; if (prompt('Type DELETE to permanently erase this account:') !== "DELETE") return; try { await request("/v1/me/account", { method: "DELETE" }); voiceClient?.hangup(); await groupVoiceClient?.leave(); if (state.socket) state.socket.close(); await clearToken(); await clearPendingRecovery(); try { await invoke("credential_delete_recovery", { serverKey: await serverKey(activeBase()) }); } catch {} state.recoveryKey = ""; state.cryptoDevice = null; state.user = null; state.blockedUserIds.clear(); state.active = null; state.friends = []; state.conversations = []; resetCryptoViewState(); $("#settingsDialog").close(); show("authView"); toast("Account and data deleted."); } catch (error) { toast(error.message, true); } };
  $("#logoutButton").onclick = async () => { try { await request("/v1/sessions", { method: "DELETE" }); } catch {} voiceClient?.hangup(); await groupVoiceClient?.leave(); if (state.socket) state.socket.close(); await clearToken(); state.recoveryKey = ""; state.cryptoDevice = null; state.user = null; state.blockedUserIds.clear(); state.active = null; state.conversations = []; resetCryptoViewState(); show("authView"); toast("You are logged out."); };
  $("#checkUpdates").onclick = () => checkForUpdates(false);
  $("#installUpdate").onclick = installUpdate;
  window.__TAURI__?.event?.listen?.("ghostchat:update-progress", event => {
    const percent = Number(event.payload);
    if (Number.isFinite(percent)) {
      $("#updateStatus").textContent = `${translate("Downloading update")}… ${Math.min(100, Math.max(0, percent))}%.`;
      $("#startupUpdateStatus").textContent = `${translate("Downloading update")}… ${Math.min(100, Math.max(0, percent))}%.`;
      $("#startupUpdateProgress").style.width = `${Math.min(100, Math.max(0, percent))}%`;
    }
  });
  window.__TAURI__?.event?.listen?.("ghostchat:encrypted-attachment-progress", event => {
    const progress = event.payload || {};
    const sent = Number(progress.sentBytes); const total = Number(progress.totalBytes);
    if (!Number.isFinite(sent) || total <= 0) return;
    const percent = Math.min(100, Math.floor(sent * 100 / total));
    if (progress.direction === "download") {
      const card = $$(".encrypted-attachment").find(item => item.dataset.attachmentId === progress.attachmentId);
      if (!card) return;
      const button = $(".encrypted-attachment-actions .attachment-download", card);
      if (button?.disabled) button.textContent = translate("Downloading and decrypting securely…") + " " + percent + "%";
      return;
    }
    if (progress.direction !== "upload") return;
    const button = $("#attachFileButton");
    if (!button.disabled) return;
    button.title = translate("Encrypting and uploading securely…") + " " + percent + "%";
  });
  document.addEventListener("keydown", event => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); setPage("search"); }
    if (event.key === "Escape" && !$("#settingsDialog").open) document.activeElement?.blur();
    if (event.code === "Space" && !event.repeat && voiceSettings().pushToTalk && !event.target.closest("input, textarea, select, [contenteditable=true], button")) {
      event.preventDefault();
      if (groupVoiceUiState.status !== "idle") void groupVoiceClient?.setPushToTalk(true).catch(error => toast(error.message, true));
      else voiceClient?.setPushToTalk(true);
    }
  });
  document.addEventListener("keyup", event => {
    if (event.code !== "Space") return;
    if (groupVoiceUiState.status !== "idle") void groupVoiceClient?.setPushToTalk(false).catch(error => toast(error.message, true));
    else voiceClient?.setPushToTalk(false);
  });
  window.addEventListener("blur", () => {
    if (groupVoiceUiState.status !== "idle") void groupVoiceClient?.setPushToTalk(false).catch(() => {});
    else voiceClient?.setPushToTalk(false);
  });
  setupVoiceClient();
  void refreshAudioDevices({ requestPermission: false, quiet: true });
  navigator.mediaDevices?.addEventListener?.("devicechange", () => { void refreshAudioDevices({ requestPermission: false, quiet: true }); });

  async function boot() {
    setTheme(); seasonalIdentity();
    try { state.localBase = await invoke("api_base"); }
    catch { state.localBase = "http://127.0.0.1:38241"; }
    if (!await startupUpdateCheck()) return;
    const savedServer = localStorage.getItem("ghostchat.server");
    if (savedServer === LOCAL_SERVER_SENTINEL) {
      state.base = "";
    } else if (!savedServer || isLocalServer(savedServer) || isLegacyDefaultServer(savedServer)) {
      const migrateLegacyCredentials = isLegacyDefaultServer(savedServer);
      state.base = DEFAULT_REMOTE_SERVER;
      localStorage.setItem("ghostchat.server", state.base);
      if (migrateLegacyCredentials) await migrateServerCredentials(LEGACY_DEFAULT_REMOTE_SERVER, DEFAULT_REMOTE_SERVER);
    } else {
      state.base = normalizedBase(savedServer);
    }
    updateServerLabels();
    await restoreToken();
    await processPendingGroupInvites();
    try { state.pendingRecovery = await invoke("credential_get_pending_recovery", { serverKey: await serverKey(activeBase()) }) || ""; } catch {}
    if (state.pendingRecovery) {
      try { await saveRecoveryKey(state.pendingRecovery); } catch (error) { console.warn("Could not promote the pending recovery key", error); }
      $("#recoveryValue").textContent = state.pendingRecovery;
      $("#recoveryConfirm").checked = false; $("#continueAfterRecovery").disabled = true; show("recoveryView");
    } else if (state.token) await enterApp(); else show("authView");
  }
  boot();
  setInterval(seasonalIdentity, 60 * 60 * 1000);
  setTimeout(() => checkForUpdates(true), 8000);
  setInterval(() => checkForUpdates(true), 6 * 60 * 60 * 1000);
})();
