import { Component, Input } from '@angular/core'
import { ModalController } from '@ionic/angular'
import { RecoveryDrive } from 'src/app/services/api/api.service'

@Component({
  selector: 'password',
  templateUrl: 'password.page.html',
  styleUrls: ['password.page.scss'],
})
export class PasswordPage {
  @Input() recoveryDrive: RecoveryDrive

  needsVer: boolean

  error = ''
  password = ''
  passwordVer = ''

  constructor(
    private modalController: ModalController
  ) {}

  ngOnInit() {
    this.needsVer = !!this.recoveryDrive && !this.recoveryDrive.version.startsWith('0.2')
  }

  async submitPassword () {
    if(!this.needsVer) {
      if (this.password.length < 12) {
        this.error="*passwords must be 12 characters or greater"
      } else if (this.password !== this.passwordVer) {
        this.error="*passwords dont match"
      }
    } else {
      this.modalController.dismiss({
        password: this.password,
      })
    }
  }

  cancel () {
    this.modalController.dismiss()
  }
}