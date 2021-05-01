import { Component } from '@angular/core'
import { LoadingOptions } from '@ionic/core'
import { ServerStatus } from 'src/app/models/server-model'
import { AlertController } from '@ionic/angular'
import { S9Server } from 'src/app/models/server-model'
import { ApiService } from 'src/app/services/api/api.service'
import { Subscription, Observable } from 'rxjs'
import { LoaderService } from 'src/app/services/loader.service'
import { PatchDbModel } from 'src/app/models/patch-db/patch-db-model'

@Component({
  selector: 'server-show',
  templateUrl: 'server-show.page.html',
  styleUrls: ['server-show.page.scss'],
})
export class ServerShowPage {
  server: Observable<S9Server>
  subsToTearDown: Subscription[] = []
  updating = false
  error = ''

  constructor (
    private readonly alertCtrl: AlertController,
    private readonly loader: LoaderService,
    private readonly apiService: ApiService,
    private readonly patch: PatchDbModel,
  ) { }

  async ngOnInit () {
    this.server = this.patch.watch$('server')

    this.subsToTearDown.push(
      // serverUpdateSubscription
      this.server.subscribe(s => {
        this.updating = s.status === ServerStatus.UPDATING
      }),
    )
  }

  ionViewDidEnter () {
    this.error = ''
  }

  ngOnDestroy () {
    this.subsToTearDown.forEach(s => s.unsubscribe())
  }

  async presentAlertRestart () {
    const alert = await this.alertCtrl.create({
      backdropDismiss: false,
      header: 'Confirm',
      message: `Are you sure you want to restart your Embassy?`,
      buttons: [
        {
          text: 'Cancel',
          role: 'cancel',
        },
        {
          text: 'Restart',
          cssClass: 'alert-danger',
          handler: () => {
            this.restart()
          },
        },
      ]},
    )
    await alert.present()
  }

  async presentAlertShutdown () {
    const alert = await this.alertCtrl.create({
      backdropDismiss: false,
      header: 'Confirm',
      message: `Are you sure you want to shut down your Embassy? To turn it back on, you will need to physically unplug the device and plug it back in.`,
      buttons: [
        {
          text: 'Cancel',
          role: 'cancel',
        },
        {
          text: 'Shutdown',
          cssClass: 'alert-danger',
          handler: () => {
            this.shutdown()
          },
        },
      ],
    })
    await alert.present()
  }

  private async restart () {
    this.loader
      .of(LoadingSpinner(`Restarting...`))
      .displayDuringAsync( async () => {
        // this.serverModel.markUnreachable()
        await this.apiService.restartServer()
      })
      .catch(e => this.setError(e))
  }

  private async shutdown () {
    this.loader
      .of(LoadingSpinner(`Shutting down...`))
      .displayDuringAsync( async () => {
        // this.serverModel.markUnreachable()
        await this.apiService.shutdownServer()
      })
      .catch(e => this.setError(e))
  }

  setError (e: Error) {
    console.error(e)
    this.error = e.message
  }
}

const LoadingSpinner: (m?: string) => LoadingOptions = (m) => {
  const toMergeIn = m ? { message: m } : { }
  return {
    spinner: 'lines',
    cssClass: 'loader',
    ...toMergeIn,
  } as LoadingOptions
}


