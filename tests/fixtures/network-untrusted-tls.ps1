# Synthetic certificate; never adds to Windows certificate stores. Its uniquely named test key is deleted in finally.
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Net;
using System.Net.Security;
using System.Net.Sockets;
using System.Security.Authentication;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Threading;
using System.Threading.Tasks;
public static class DIYIME2UntrustedTlsFixture {
    static SslServerAuthenticationOptions Options(X509Certificate2 certificate) {
        return new SslServerAuthenticationOptions { ServerCertificate = certificate, EnabledSslProtocols = SslProtocols.Tls12, ClientCertificateRequired = false, CertificateRevocationCheckMode = X509RevocationMode.NoCheck };
    }
    public static async Task RunAsync() {
        var parameters = new CngKeyCreationParameters { Provider = CngProvider.MicrosoftSoftwareKeyStorageProvider, KeyUsage = CngKeyUsages.AllUsages };
        parameters.Parameters.Add(new CngProperty("Length", BitConverter.GetBytes(2048), CngPropertyOptions.None));
        string keyName = "DIYIME2.TestTLS." + Guid.NewGuid().ToString("N");
        using (var testKey = CngKey.Create(CngAlgorithm.Rsa, keyName, parameters)) {
          try {
           using (RSA key = new RSACng(testKey)) {
            var request = new CertificateRequest("CN=localhost", key, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);
            var names = new SubjectAlternativeNameBuilder();names.AddDnsName("localhost");names.AddIpAddress(IPAddress.Loopback);request.CertificateExtensions.Add(names.Build());
            using (var certificate = request.CreateSelfSigned(DateTimeOffset.UtcNow.AddMinutes(-1), DateTimeOffset.UtcNow.AddHours(1))) {
                var listener = new TcpListener(IPAddress.Loopback, 0);listener.Start();
                using (var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10))) {
                    try {
                        int port = ((IPEndPoint)listener.LocalEndpoint).Port;
                        // Prove this ephemeral certificate can actually serve TLS. The
                        // fixture-only client pins this exact certificate in memory.
                        Task self = Task.Run(async () => {
                            using (var client = new TcpClient()) {
                                await client.ConnectAsync(IPAddress.Loopback, port, timeout.Token);
                                using (var tls = new SslStream(client.GetStream(), false, (sender, peer, chain, errors) => peer != null && peer.GetCertHashString() == certificate.Thumbprint)) {
                                    await tls.AuthenticateAsClientAsync(new SslClientAuthenticationOptions { TargetHost = "localhost", EnabledSslProtocols = SslProtocols.Tls12, CertificateRevocationCheckMode = X509RevocationMode.NoCheck }, timeout.Token);
                                }
                            }
                        });
                        using (var client = await listener.AcceptTcpClientAsync(timeout.Token))
                        using (var tls = new SslStream(client.GetStream(), false)) { await tls.AuthenticateAsServerAsync(Options(certificate), timeout.Token); }
                        await self;
                        Console.WriteLine(port);Console.WriteLine("self-check-ok");Console.Out.Flush();
                        Task stop = Task.Run(async () => { await Console.In.ReadLineAsync();timeout.Cancel(); });
                        while (!timeout.IsCancellationRequested) {
                            try {
                                using (var client = await listener.AcceptTcpClientAsync(timeout.Token))
                                using (var tls = new SslStream(client.GetStream(), false)) {
                                                                        try {
                                        await tls.AuthenticateAsServerAsync(Options(certificate), timeout.Token);
                                        Console.WriteLine("tls-established");
                                        byte[] application = new byte[1];
                                        int count = await tls.ReadAsync(application.AsMemory(), timeout.Token);
                                        Console.WriteLine(count == 0 ? "no-http" : "http-received");
                                    }
                                    catch (AuthenticationException) { Console.WriteLine("rejected"); }
                                    catch (IOException) { Console.WriteLine("rejected"); }
                                    Console.Out.Flush();
                                }
                            } catch (OperationCanceledException) { break; }
                        }
                    } finally { listener.Stop(); }
                }
            }
        }
          } finally {
              testKey.Delete();
              if (CngKey.Exists(keyName, CngProvider.MicrosoftSoftwareKeyStorageProvider)) throw new InvalidOperationException("owned test key was not deleted");
          }
        }
    }
}
"@
[DIYIME2UntrustedTlsFixture]::RunAsync().GetAwaiter().GetResult()
