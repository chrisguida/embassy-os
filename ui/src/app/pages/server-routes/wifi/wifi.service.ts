import { Injectable } from '@angular/core'
import { AlertController, ToastController } from '@ionic/angular'
import { merge, Observable, timer } from 'rxjs'
import { filter, map, take, tap } from 'rxjs/operators'
import { PatchDbModel } from 'src/app/models/patch-db/patch-db-model'

@Injectable({
  providedIn: 'root',
})
export class WifiService {

  constructor (
    private readonly toastCtrl: ToastController,
    private readonly alertCtrl: AlertController,
    private readonly patch: PatchDbModel,
  ) { }

  confirmWifi (ssid: string): Observable<boolean> {
    const success$ = this.patch.watch$('server', 'wifi', 'current')
    .pipe(
      filter(current => current === ssid),
      tap(current => this.presentAlertSuccess(current)),
      map(_ => true),
    )

    const timer$ = timer(20000)
    .pipe(
      map(_ => false),
      tap(_ => this.presentToastFail()),
    )

    return merge(success$, timer$).pipe(take(1))
  }

  async presentToastFail (): Promise<void> {
    const toast = await this.toastCtrl.create({
      header: 'Failed to connect:',
      message: `Check credentials and try again`,
      position: 'bottom',
      duration: 4000,
      buttons: [
        {
          side: 'start',
          icon: 'close',
          handler: () => {
            return true
          },
        },
      ],
      cssClass: 'notification-toast-error',
    })

    await toast.present()
  }

  async presentAlertSuccess (current: string): Promise<void> {
    const alert = await this.alertCtrl.create({
      header: `Connected to "${current}"`,
      message: 'Note. It may take several minutes to an hour for your Embassy to reconnect over Tor.',
      buttons: ['OK'],
    })

    await alert.present()
  }
}
